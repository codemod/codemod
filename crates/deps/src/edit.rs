//! Format-preserving edits to manifests: package.json (and other JSON
//! objects) and the YAML catalogs of pnpm-workspace.yaml and .yarnrc.yml.
//!
//! Only the value, key or line that changes is touched: indentation, key
//! order, quotes and the trailing newline stay as they are. Pure functions
//! over text, so the CLI and Grepkon can share them.

/// A member of a JSON object, by byte offsets into the text.
#[derive(Debug, Clone)]
struct Member {
    key: String,
    /// The key, quotes included.
    key_start: usize,
    value_start: usize,
    value_end: usize,
}

/// A JSON object: its braces and members.
#[derive(Debug, Clone)]
struct Object {
    open: usize,
    close: usize,
    members: Vec<Member>,
}

struct Scanner<'a> {
    text: &'a [u8],
}

impl Scanner<'_> {
    fn skip_space(&self, mut at: usize) -> usize {
        while at < self.text.len() && matches!(self.text[at], b' ' | b'\t' | b'\n' | b'\r') {
            at += 1;
        }
        at
    }

    /// The end of the string starting at `at` (a quote), after its closing quote.
    fn string_end(&self, at: usize) -> Result<usize, String> {
        let mut i = at + 1;
        while i < self.text.len() {
            match self.text[i] {
                b'\\' => i += 2,
                b'"' => return Ok(i + 1),
                _ => i += 1,
            }
        }
        Err("unterminated string".into())
    }

    /// The end of the value starting at `at`.
    fn value_end(&self, at: usize) -> Result<usize, String> {
        match self.text.get(at) {
            Some(b'"') => self.string_end(at),
            Some(b'{') | Some(b'[') => {
                let mut depth = 0usize;
                let mut i = at;
                while i < self.text.len() {
                    match self.text[i] {
                        b'"' => {
                            i = self.string_end(i)?;
                            continue;
                        }
                        b'{' | b'[' => depth += 1,
                        b'}' | b']' => {
                            depth -= 1;
                            if depth == 0 {
                                return Ok(i + 1);
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                Err("unterminated object or array".into())
            }
            Some(_) => {
                let mut i = at;
                while i < self.text.len()
                    && !matches!(
                        self.text[i],
                        b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r'
                    )
                {
                    i += 1;
                }
                if i == at {
                    return Err(format!("expected a value at byte {at}"));
                }
                Ok(i)
            }
            None => Err("unexpected end of JSON".into()),
        }
    }

    fn object(&self, open: usize) -> Result<Object, String> {
        if self.text.get(open) != Some(&b'{') {
            return Err(format!("expected an object at byte {open}"));
        }
        let mut members = Vec::new();
        let mut i = self.skip_space(open + 1);
        loop {
            match self.text.get(i) {
                Some(b'}') => {
                    return Ok(Object {
                        open,
                        close: i,
                        members,
                    })
                }
                Some(b'"') => {
                    let key_end = self.string_end(i)?;
                    let key: String = serde_json::from_slice(&self.text[i..key_end])
                        .map_err(|e| format!("bad key: {e}"))?;
                    let colon = self.skip_space(key_end);
                    if self.text.get(colon) != Some(&b':') {
                        return Err(format!("expected ':' after \"{key}\""));
                    }
                    let value_start = self.skip_space(colon + 1);
                    let value_end = self.value_end(value_start)?;
                    members.push(Member {
                        key,
                        key_start: i,
                        value_start,
                        value_end,
                    });
                    i = self.skip_space(value_end);
                    if self.text.get(i) == Some(&b',') {
                        i = self.skip_space(i + 1);
                    }
                }
                Some(other) => {
                    return Err(format!("unexpected '{}' at byte {i}", char::from(*other)))
                }
                None => return Err("unterminated object".into()),
            }
        }
    }
}

/// The object at `path` (keys from the root object), if every key is there
/// and holds an object.
fn find_object(text: &str, path: &[&str]) -> Result<Option<Object>, String> {
    let scanner = Scanner {
        text: text.as_bytes(),
    };
    let mut object = scanner.object(scanner.skip_space(0))?;
    for key in path {
        let Some(member) = object.members.iter().find(|m| m.key == *key) else {
            return Ok(None);
        };
        if text.as_bytes().get(member.value_start) != Some(&b'{') {
            return Ok(None);
        }
        object = scanner.object(member.value_start)?;
    }
    Ok(Some(object))
}

fn newline(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// The whitespace before `at` on its line.
fn indent_before(text: &str, at: usize) -> Option<&str> {
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let indent = &text[line_start..at];
    indent
        .chars()
        .all(|c| c == ' ' || c == '\t')
        .then_some(indent)
}

/// One level of indentation, as the file uses it.
fn indent_unit(text: &str) -> String {
    for line in text.lines().skip(1) {
        let indent: String = line
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        if !indent.is_empty() {
            return indent;
        }
    }
    "  ".into()
}

fn quote(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| format!("\"{text}\""))
}

fn is_sorted(members: &[Member]) -> bool {
    members.windows(2).all(|pair| pair[0].key <= pair[1].key)
}

/// Where a new key goes: sorted position when the keys are sorted, else last.
fn insert_position(members: &[Member], key: &str) -> usize {
    if members.len() > 1 && is_sorted(members) || members.len() == 1 {
        members
            .iter()
            .position(|m| m.key.as_str() > key)
            .unwrap_or(members.len())
    } else {
        members.len()
    }
}

/// Insert `"key": value_json` into `object` at `position`.
fn insert_member(
    text: &str,
    object: &Object,
    position: usize,
    key: &str,
    value_json: &str,
) -> String {
    let nl = newline(text);
    let entry = format!("{}: {value_json}", quote(key));
    let members = &object.members;
    if members.is_empty() {
        let outer = indent_before(text, object.open)
            .map(str::to_string)
            .unwrap_or_else(|| {
                // The object opens after a key: its line's indent.
                let line_start = text[..object.open].rfind('\n').map_or(0, |i| i + 1);
                text[line_start..]
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .collect()
            });
        let inner = format!("{outer}{}", indent_unit(text));
        return format!(
            "{}{{{nl}{inner}{entry}{nl}{outer}}}{}",
            &text[..object.open],
            &text[object.close + 1..]
        );
    }
    let single_line = !text[object.open..object.close].contains('\n');
    if single_line {
        return if position == members.len() {
            let end = members[position - 1].value_end;
            format!("{}, {entry}{}", &text[..end], &text[end..])
        } else {
            let start = members[position].key_start;
            format!("{}{entry}, {}", &text[..start], &text[start..])
        };
    }
    let indent = indent_before(text, members[0].key_start)
        .unwrap_or("  ")
        .to_string();
    if position == members.len() {
        let end = members[position - 1].value_end;
        format!("{},{nl}{indent}{entry}{}", &text[..end], &text[end..])
    } else {
        let start = members[position].key_start;
        format!("{}{entry},{nl}{indent}{}", &text[..start], &text[start..])
    }
}

/// Remove member `index` of `object`, with its comma.
fn remove_member(text: &str, object: &Object, index: usize) -> String {
    let members = &object.members;
    let member = &members[index];
    if members.len() == 1 {
        return format!("{}{}", &text[..object.open + 1], &text[object.close..]);
    }
    if index + 1 < members.len() {
        let next = members[index + 1].key_start;
        // From this key to the next one: the next keeps this one's indent.
        format!("{}{}", &text[..member.key_start], &text[next..])
    } else {
        let previous = members[index - 1].value_end;
        format!("{}{}", &text[..previous], &text[member.value_end..])
    }
}

/// Set `key` in the object at `path` to the string `value`. When the object
/// is missing, it is created (one level) after `after`'s last present key,
/// or at the end. Returns the new text and the old value, if any.
pub fn json_set(
    text: &str,
    path: &[&str],
    key: &str,
    value: &str,
    after: &[&str],
) -> Result<(String, Option<String>), String> {
    match find_object(text, path)? {
        Some(object) => {
            if let Some(member) = object.members.iter().find(|m| m.key == key) {
                let old: Option<String> =
                    serde_json::from_str(&text[member.value_start..member.value_end]).ok();
                let edited = format!(
                    "{}{}{}",
                    &text[..member.value_start],
                    quote(value),
                    &text[member.value_end..]
                );
                return Ok((edited, old));
            }
            let position = insert_position(&object.members, key);
            Ok((
                insert_member(text, &object, position, key, &quote(value)),
                None,
            ))
        }
        None => {
            let (name, parent_path) = path.split_last().ok_or("the document is not an object")?;
            let parent = find_object(text, parent_path)?
                .ok_or_else(|| format!("no object at {}", parent_path.join(".")))?;
            let position = after
                .iter()
                .filter_map(|section| parent.members.iter().position(|m| m.key == *section))
                .max()
                .map_or(parent.members.len(), |index| index + 1);
            // The new object is written one line per member, as the file indents.
            let unit = indent_unit(text);
            let indent = parent
                .members
                .first()
                .and_then(|m| indent_before(text, m.key_start))
                .unwrap_or(&unit)
                .to_string();
            let nl = newline(text);
            let object_json = format!(
                "{{{nl}{indent}{unit}{}: {}{nl}{indent}}}",
                quote(key),
                quote(value)
            );
            Ok((
                insert_member(text, &parent, position, name, &object_json),
                None,
            ))
        }
    }
}

/// Remove `key` from the object at `path`. Returns the new text and the old
/// value; the text is unchanged when the key is not there.
pub fn json_remove(
    text: &str,
    path: &[&str],
    key: &str,
) -> Result<(String, Option<String>), String> {
    let Some(object) = find_object(text, path)? else {
        return Ok((text.to_string(), None));
    };
    let Some(index) = object.members.iter().position(|m| m.key == key) else {
        return Ok((text.to_string(), None));
    };
    let member = &object.members[index];
    let old: Option<String> =
        serde_json::from_str(&text[member.value_start..member.value_end]).ok();
    Ok((remove_member(text, &object, index), old))
}

/// Rename `from` to `to` in the object at `path`, setting its value to
/// `value` when given. A sorted object stays sorted; otherwise the key keeps
/// its place. Returns the new text and the old value.
pub fn json_rename(
    text: &str,
    path: &[&str],
    from: &str,
    to: &str,
    value: Option<&str>,
) -> Result<(String, Option<String>), String> {
    let Some(object) = find_object(text, path)? else {
        return Ok((text.to_string(), None));
    };
    let Some(index) = object.members.iter().position(|m| m.key == from) else {
        return Ok((text.to_string(), None));
    };
    let member = &object.members[index];
    let old: Option<String> =
        serde_json::from_str(&text[member.value_start..member.value_end]).ok();
    let new_value = match value {
        Some(value) => quote(value),
        None => text[member.value_start..member.value_end].to_string(),
    };
    let sorted = object.members.len() > 1 && is_sorted(&object.members);
    let others: Vec<Member> = object
        .members
        .iter()
        .filter(|m| m.key != from)
        .cloned()
        .collect();
    let stays_sorted = {
        let at = insert_position(&others, to);
        at == index
    };
    if !sorted || stays_sorted {
        let key_end = Scanner {
            text: text.as_bytes(),
        }
        .string_end(member.key_start)?;
        let edited = format!(
            "{}{}{}{}{}",
            &text[..member.key_start],
            quote(to),
            &text[key_end..member.value_start],
            new_value,
            &text[member.value_end..]
        );
        return Ok((edited, old));
    }
    // Sorted, and the new name sorts elsewhere: remove, then insert sorted.
    let removed = remove_member(text, &object, index);
    let object = find_object(&removed, path)?.ok_or("the object went missing")?;
    let position = insert_position(&object.members, to);
    Ok((
        insert_member(&removed, &object, position, to, &new_value),
        old,
    ))
}

/// The string members of the object at `path`, in order.
pub fn json_entries(text: &str, path: &[&str]) -> Result<Vec<(String, String)>, String> {
    let Some(object) = find_object(text, path)? else {
        return Ok(Vec::new());
    };
    Ok(object
        .members
        .iter()
        .filter_map(|m| {
            let value: String = serde_json::from_str(&text[m.value_start..m.value_end]).ok()?;
            Some((m.key.clone(), value))
        })
        .collect())
}

// ------------------------------------------------------------------ YAML ---

/// A YAML mapping found by lines: the key line, and its entries' lines.
struct Block {
    /// Index of the line after the key line; entries start here.
    start: usize,
    /// Index after the last entry line.
    end: usize,
    /// Indent of the entries, when there are any.
    entry_indent: Option<usize>,
    /// Indent of the key line.
    indent: usize,
}

fn line_indent(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ').count()
}

fn is_blank_or_comment(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.is_empty() || trimmed.starts_with('#')
}

/// `key:` alone on a line (a block mapping follows), with its key unquoted.
fn block_key(line: &str) -> Option<String> {
    let trimmed = line.trim_end();
    let content = trimmed.trim_start();
    let without_comment = match content.find(" #") {
        Some(at) => content[..at].trim_end(),
        None => content,
    };
    let key = without_comment.strip_suffix(':')?;
    Some(unquote(key.trim()))
}

fn unquote(text: &str) -> String {
    let t = text.trim();
    if t.len() >= 2
        && ((t.starts_with('\'') && t.ends_with('\'')) || (t.starts_with('"') && t.ends_with('"')))
    {
        t[1..t.len() - 1].to_string()
    } else {
        t.to_string()
    }
}

/// The block of `key` among the children of `parent` (or at the top level).
fn find_block(lines: &[&str], parent: Option<&Block>, key: &str) -> Option<Block> {
    let (from, to, indent) = match parent {
        Some(parent) => (parent.start, parent.end, parent.entry_indent?),
        None => (0, lines.len(), 0),
    };
    for i in from..to {
        let line = lines[i];
        if is_blank_or_comment(line) || line_indent(line) != indent {
            continue;
        }
        if block_key(line).as_deref() != Some(key) {
            continue;
        }
        let mut end = i + 1;
        let mut entry_indent = None;
        let mut last = i + 1;
        while end < lines.len() {
            let next = lines[end];
            if is_blank_or_comment(next) {
                end += 1;
                continue;
            }
            let next_indent = line_indent(next);
            if next_indent <= indent {
                break;
            }
            entry_indent.get_or_insert(next_indent);
            end += 1;
            last = end;
        }
        return Some(Block {
            start: i + 1,
            end: last,
            entry_indent,
            indent,
        });
    }
    None
}

/// `key: value` on one line: (indent, key, value text as written, comment).
fn entry(line: &str) -> Option<(usize, String, String)> {
    let indent = line_indent(line);
    let content = &line[indent..];
    let (key, rest) = if let Some(quoted) = content.strip_prefix('\'') {
        let end = quoted.find('\'')?;
        (quoted[..end].to_string(), &quoted[end + 1..])
    } else if let Some(quoted) = content.strip_prefix('"') {
        let end = quoted.find('"')?;
        (quoted[..end].to_string(), &quoted[end + 1..])
    } else {
        let colon = content.find(':')?;
        (content[..colon].trim().to_string(), &content[colon..])
    };
    let value = rest.trim_start().strip_prefix(':')?.trim();
    Some((indent, key, value.to_string()))
}

/// A YAML scalar's text as written: quotes removed, comment dropped.
fn scalar(text: &str) -> String {
    let t = text.trim();
    if t.starts_with('\'') || t.starts_with('"') {
        let q = &t[..1];
        if let Some(end) = t[1..].find(q) {
            return t[1..1 + end].to_string();
        }
    }
    match t.find(" #") {
        Some(at) => t[..at].trim().to_string(),
        None => t.to_string(),
    }
}

/// `value` as a YAML scalar, quoted the way `like` (the old value) was, or
/// with single quotes when YAML would read it as something else.
fn yaml_scalar(value: &str, like: Option<&str>) -> String {
    let quote_char = like.and_then(|old| {
        let t = old.trim();
        t.chars().next().filter(|c| *c == '\'' || *c == '"')
    });
    let needs_quotes = value.is_empty()
        || value == "~"
        || value.chars().next().is_some_and(|c| {
            "*&!|>'\"%@`{[#,?-:".contains(c)
                && !(c == '-' && value.len() > 1 && value.as_bytes()[1].is_ascii_digit())
        })
        || value.contains(": ")
        || value.contains(" #");
    match quote_char {
        Some('"') => quote(value),
        Some(_) => format!("'{}'", value.replace('\'', "''")),
        None if needs_quotes => format!("'{}'", value.replace('\'', "''")),
        None => value.to_string(),
    }
}

fn yaml_key(key: &str) -> String {
    if key.starts_with('@') || key.contains(':') || key.contains(' ') || key.contains('#') {
        format!("'{key}'")
    } else {
        key.to_string()
    }
}

/// The path of a YAML catalog: `catalog` for the default one, or
/// `catalogs` then its name.
fn catalog_block(lines: &[&str], catalog: &str) -> Option<Block> {
    if catalog == "default" {
        if let Some(block) = find_block(lines, None, "catalog") {
            return Some(block);
        }
    }
    let catalogs = find_block(lines, None, "catalogs")?;
    find_block(lines, Some(&catalogs), catalog)
}

/// The entries of a catalog in a YAML file: `catalog` (default) or
/// `catalogs.<name>`, with every named catalog when `catalog` is None.
pub fn yaml_catalogs(text: &str) -> Vec<(String, Vec<(String, String)>)> {
    let lines: Vec<&str> = text.lines().collect();
    let read = |block: &Block| -> Vec<(String, String)> {
        (block.start..block.end)
            .filter_map(|i| {
                let line = lines[i];
                if is_blank_or_comment(line) || Some(line_indent(line)) != block.entry_indent {
                    return None;
                }
                let (_, key, value) = entry(line)?;
                Some((key, scalar(&value)))
            })
            .collect()
    };
    let mut found = Vec::new();
    if let Some(block) = find_block(&lines, None, "catalog") {
        found.push(("default".to_string(), read(&block)));
    }
    if let Some(catalogs) = find_block(&lines, None, "catalogs") {
        if let Some(indent) = catalogs.entry_indent {
            for i in catalogs.start..catalogs.end {
                let line = lines[i];
                if is_blank_or_comment(line) || line_indent(line) != indent {
                    continue;
                }
                if let Some(name) = block_key(line) {
                    if let Some(block) = find_block(&lines, Some(&catalogs), &name) {
                        found.push((name, read(&block)));
                    }
                }
            }
        }
    }
    found
}

/// The `packages` globs of a pnpm-workspace.yaml.
pub fn yaml_list(text: &str, key: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let Some(block) = find_block(&lines, None, key) else {
        return Vec::new();
    };
    (block.start..block.end)
        .filter_map(|i| {
            let item = lines[i].trim().strip_prefix("- ")?;
            Some(scalar(item))
        })
        .collect()
}

fn join_lines(text: &str, lines: &[String]) -> String {
    let nl = newline(text);
    let mut joined = lines.join(nl);
    if text.ends_with('\n') {
        joined.push_str(nl);
    }
    joined
}

/// Set `name` in a YAML catalog to `range` (added when missing). Returns the
/// new text and the old range.
pub fn yaml_catalog_set(
    text: &str,
    catalog: &str,
    name: &str,
    range: &str,
) -> Result<(String, Option<String>), String> {
    let lines: Vec<&str> = text.lines().collect();
    let block = catalog_block(&lines, catalog).ok_or_else(|| {
        format!("no catalog '{catalog}' (or it is written inline, which is not edited)")
    })?;
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    for i in block.start..block.end {
        let line = lines[i];
        if Some(line_indent(line)) != block.entry_indent {
            continue;
        }
        if let Some((indent, key, value)) = entry(line) {
            if key == name {
                let colon_at = line[indent..]
                    .find(&value[..])
                    .map(|v| indent + v)
                    .unwrap_or(line.len());
                let comment = match value.chars().next() {
                    Some(q @ ('\'' | '"')) => {
                        value[1..].find(q).map_or("", |end| &value[end + 2..])
                    }
                    _ => value.find(" #").map_or("", |at| &value[at..]),
                };
                out[i] = format!(
                    "{}{}{comment}",
                    &line[..colon_at],
                    yaml_scalar(range, Some(&value))
                );
                return Ok((join_lines(text, &out), Some(scalar(&value))));
            }
        }
    }
    let indent = " ".repeat(block.entry_indent.unwrap_or(block.indent + 2));
    out.insert(
        block.end,
        format!("{indent}{}: {}", yaml_key(name), yaml_scalar(range, None)),
    );
    Ok((join_lines(text, &out), None))
}

/// Remove `name` from a YAML catalog. Returns the new text and the old range.
pub fn yaml_catalog_remove(
    text: &str,
    catalog: &str,
    name: &str,
) -> Result<(String, Option<String>), String> {
    let lines: Vec<&str> = text.lines().collect();
    let Some(block) = catalog_block(&lines, catalog) else {
        return Ok((text.to_string(), None));
    };
    for i in block.start..block.end {
        if Some(line_indent(lines[i])) != block.entry_indent {
            continue;
        }
        if let Some((_, key, value)) = entry(lines[i]) {
            if key == name {
                let out: Vec<String> = lines
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| *j != i)
                    .map(|(_, l)| l.to_string())
                    .collect();
                return Ok((join_lines(text, &out), Some(scalar(&value))));
            }
        }
    }
    Ok((text.to_string(), None))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACKAGE: &str = "{\n  \"name\": \"app\",\n  \"dependencies\": {\n    \"left-pad\": \"^1.0.0\",\n    \"react\": \"^18.2.0\"\n  },\n  \"devDependencies\": {\n    \"typescript\": \"^5.0.0\"\n  }\n}\n";

    #[test]
    fn setting_a_value_touches_only_the_value() {
        let (text, old) = json_set(PACKAGE, &["dependencies"], "react", "^19.0.0", &[]).unwrap();
        assert_eq!(old.as_deref(), Some("^18.2.0"));
        assert_eq!(text, PACKAGE.replace("^18.2.0", "^19.0.0"));
    }

    #[test]
    fn a_new_key_goes_in_sorted_position() {
        let (text, old) = json_set(PACKAGE, &["dependencies"], "moment", "^2.29.4", &[]).unwrap();
        assert_eq!(old, None);
        assert_eq!(
            text,
            PACKAGE.replace(
                "\"react\": \"^18.2.0\"",
                "\"moment\": \"^2.29.4\",\n    \"react\": \"^18.2.0\""
            )
        );
        let (text, _) = json_set(PACKAGE, &["dependencies"], "zod", "^3.0.0", &[]).unwrap();
        assert!(
            text.contains("\"react\": \"^18.2.0\",\n    \"zod\": \"^3.0.0\"\n  },"),
            "{text}"
        );
        let (text, _) = json_set(PACKAGE, &["dependencies"], "a", "1", &[]).unwrap();
        assert!(
            text.contains("{\n    \"a\": \"1\",\n    \"left-pad\""),
            "{text}"
        );
    }

    #[test]
    fn an_unsorted_object_gets_new_keys_last() {
        let text = "{\n  \"dependencies\": {\n    \"z\": \"1\",\n    \"a\": \"1\"\n  }\n}";
        let (text, _) = json_set(text, &["dependencies"], "m", "2", &[]).unwrap();
        assert_eq!(text, "{\n  \"dependencies\": {\n    \"z\": \"1\",\n    \"a\": \"1\",\n    \"m\": \"2\"\n  }\n}");
    }

    #[test]
    fn a_missing_section_is_created_after_the_named_ones() {
        let text = "{\n  \"name\": \"app\",\n  \"dependencies\": {\n    \"a\": \"1\"\n  },\n  \"scripts\": {}\n}\n";
        let (text, _) = json_set(
            text,
            &["devDependencies"],
            "vitest",
            "^3.0.0",
            &["dependencies"],
        )
        .unwrap();
        assert_eq!(
            text,
            "{\n  \"name\": \"app\",\n  \"dependencies\": {\n    \"a\": \"1\"\n  },\n  \"devDependencies\": {\n    \"vitest\": \"^3.0.0\"\n  },\n  \"scripts\": {}\n}\n"
        );
    }

    #[test]
    fn an_empty_section_gets_its_first_key_on_its_own_line() {
        let text = "{\n  \"dependencies\": {}\n}\n";
        let (text, _) = json_set(text, &["dependencies"], "a", "1", &[]).unwrap();
        assert_eq!(text, "{\n  \"dependencies\": {\n    \"a\": \"1\"\n  }\n}\n");
    }

    #[test]
    fn removing_keeps_commas_right() {
        let (text, old) = json_remove(PACKAGE, &["dependencies"], "left-pad").unwrap();
        assert_eq!(old.as_deref(), Some("^1.0.0"));
        assert!(
            text.contains("\"dependencies\": {\n    \"react\": \"^18.2.0\"\n  },"),
            "{text}"
        );
        let (text, _) = json_remove(PACKAGE, &["dependencies"], "react").unwrap();
        assert!(
            text.contains("\"dependencies\": {\n    \"left-pad\": \"^1.0.0\"\n  },"),
            "{text}"
        );
        let (text, _) = json_remove(PACKAGE, &["devDependencies"], "typescript").unwrap();
        assert!(text.contains("\"devDependencies\": {}\n}"), "{text}");
        assert!(serde_json::from_str::<serde_json::Value>(&text).is_ok());
    }

    #[test]
    fn renaming_keeps_sorted_objects_sorted() {
        let (text, old) = json_rename(
            PACKAGE,
            &["dependencies"],
            "react",
            "preact",
            Some("^10.0.0"),
        )
        .unwrap();
        assert_eq!(old.as_deref(), Some("^18.2.0"));
        assert!(
            text.contains("\"left-pad\": \"^1.0.0\",\n    \"preact\": \"^10.0.0\"\n"),
            "{text}"
        );
        let (text, _) = json_rename(PACKAGE, &["dependencies"], "react", "a-react", None).unwrap();
        assert!(
            text.contains("{\n    \"a-react\": \"^18.2.0\",\n    \"left-pad\": \"^1.0.0\"\n  }"),
            "{text}"
        );
    }

    #[test]
    fn crlf_and_tabs_are_kept() {
        let text = "{\r\n\t\"dependencies\": {\r\n\t\t\"a\": \"1\"\r\n\t}\r\n}\r\n";
        let (text, _) = json_set(text, &["dependencies"], "b", "2", &[]).unwrap();
        assert_eq!(
            text,
            "{\r\n\t\"dependencies\": {\r\n\t\t\"a\": \"1\",\r\n\t\t\"b\": \"2\"\r\n\t}\r\n}\r\n"
        );
    }

    #[test]
    fn nested_objects_are_found() {
        let text = "{\n  \"workspaces\": {\n    \"packages\": [\"packages/*\"],\n    \"catalog\": {\n      \"react\": \"^18.2.0\"\n    }\n  }\n}\n";
        let (text, old) =
            json_set(text, &["workspaces", "catalog"], "react", "^19.0.0", &[]).unwrap();
        assert_eq!(old.as_deref(), Some("^18.2.0"));
        assert!(text.contains("\"react\": \"^19.0.0\""));
        assert_eq!(
            json_entries(&text, &["workspaces", "catalog"]).unwrap(),
            [("react".to_string(), "^19.0.0".to_string())]
        );
    }

    const WORKSPACE: &str = "packages:\n  - packages/*\n  - '!packages/old'\n\ncatalog:\n  react: ^18.2.0\n  '@types/node': \"^22.0.0\" # pinned\n\ncatalogs:\n  react17:\n    react: ^17.0.2\n  legacy:\n    moment: 2.29.4\n";

    #[test]
    fn yaml_catalogs_and_lists_are_read() {
        assert_eq!(
            yaml_list(WORKSPACE, "packages"),
            ["packages/*", "!packages/old"]
        );
        let catalogs = yaml_catalogs(WORKSPACE);
        assert_eq!(
            catalogs,
            [
                (
                    "default".to_string(),
                    vec![
                        ("react".to_string(), "^18.2.0".to_string()),
                        ("@types/node".to_string(), "^22.0.0".to_string())
                    ]
                ),
                (
                    "react17".to_string(),
                    vec![("react".to_string(), "^17.0.2".to_string())]
                ),
                (
                    "legacy".to_string(),
                    vec![("moment".to_string(), "2.29.4".to_string())]
                ),
            ]
        );
    }

    #[test]
    fn a_yaml_catalog_entry_keeps_its_quotes_and_comment() {
        let (text, old) = yaml_catalog_set(WORKSPACE, "default", "@types/node", "^24.0.0").unwrap();
        assert_eq!(old.as_deref(), Some("^22.0.0"));
        assert!(
            text.contains("  '@types/node': \"^24.0.0\" # pinned\n"),
            "{text}"
        );
        let (text, old) = yaml_catalog_set(WORKSPACE, "react17", "react", "^17.0.3").unwrap();
        assert_eq!(old.as_deref(), Some("^17.0.2"));
        assert!(text.contains("  react17:\n    react: ^17.0.3\n"), "{text}");
        let (text, _) = yaml_catalog_set(WORKSPACE, "default", "react-dom", "^18.2.0").unwrap();
        assert!(
            text.contains(
                "  '@types/node': \"^22.0.0\" # pinned\n  react-dom: ^18.2.0\n\ncatalogs:"
            ),
            "{text}"
        );
        let (text, _) = yaml_catalog_set(WORKSPACE, "legacy", "x", "*").unwrap();
        assert!(text.ends_with("    moment: 2.29.4\n    x: '*'\n"), "{text}");
        let (text, old) = yaml_catalog_remove(WORKSPACE, "default", "react").unwrap();
        assert_eq!(old.as_deref(), Some("^18.2.0"));
        assert!(text.contains("catalog:\n  '@types/node'"), "{text}");
    }
}
