//! A small, deterministic printer for the TypeScript the translation
//! writes, in the style of Prettier's defaults: 80 columns, two spaces,
//! trailing commas where a list breaks.
//!
//! It works on what the generator emits, one statement per line: a line's
//! brackets are groups whose items are separated by commas, or by
//! semicolons for a block (`(file) => { a; b; }`), which always breaks. A
//! group stays on one line when it fits; otherwise a call's last argument
//! that is a function, an object or an array hugs the parentheses when the
//! rest fits, and failing that every item goes on its own line.
//!
//! Lines it cannot read (an unbalanced bracket, a string or template that
//! spans lines, a comment) are kept as they are.

const WIDTH: usize = 80;

/// `source` with its long lines and blocks broken.
pub(crate) fn format(source: &str) -> String {
    let whole = whole_lines(source);
    let mut out = String::with_capacity(source.len());
    for (index, line) in source.split_inclusive('\n').enumerate() {
        let (text, newline) = match line.strip_suffix('\n') {
            Some(text) => (text, "\n"),
            None => (line, ""),
        };
        let formatted = whole
            .get(index)
            .copied()
            .unwrap_or(false)
            .then(|| format_line(text))
            .flatten();
        out.push_str(formatted.as_deref().unwrap_or(text));
        out.push_str(newline);
    }
    out
}

/// For each line, whether it starts and ends outside any string, template
/// literal or comment: a line inside a template that spans lines (a shell
/// script) is text, not code.
fn whole_lines(source: &str) -> Vec<bool> {
    #[derive(Clone, Copy, PartialEq)]
    enum Context {
        /// Code, with how many braces it has open.
        Code(usize),
        Template,
    }
    #[derive(Clone, Copy, PartialEq)]
    enum Inside {
        Nothing,
        Quote(char),
        LineComment,
        BlockComment,
    }
    let chars: Vec<char> = source.chars().collect();
    let mut stack = vec![Context::Code(0)];
    let mut inside = Inside::Nothing;
    let mut lines = Vec::new();
    let at_top =
        |stack: &Vec<Context>, inside: Inside| stack.len() == 1 && inside == Inside::Nothing;
    let mut started = true;
    let mut index = 0;
    while index < chars.len() {
        let c = chars[index];
        let next = chars.get(index + 1).copied();
        if c == '\n' {
            if matches!(inside, Inside::LineComment | Inside::Quote(_)) {
                inside = Inside::Nothing;
            }
            lines.push(started && at_top(&stack, inside));
            started = at_top(&stack, inside);
            index += 1;
            continue;
        }
        match (inside, *stack.last().expect("never empty")) {
            (Inside::Quote(quote), _) => {
                if c == '\\' {
                    index += 1;
                } else if c == quote {
                    inside = Inside::Nothing;
                }
            }
            (Inside::LineComment, _) => {}
            (Inside::BlockComment, _) => {
                if c == '*' && next == Some('/') {
                    inside = Inside::Nothing;
                    index += 1;
                }
            }
            (Inside::Nothing, Context::Template) => match c {
                '\\' => index += 1,
                '`' => {
                    stack.pop();
                }
                '$' if next == Some('{') => {
                    stack.push(Context::Code(0));
                    index += 1;
                }
                _ => {}
            },
            (Inside::Nothing, Context::Code(depth)) => match c {
                '"' | '\'' => inside = Inside::Quote(c),
                '/' if next == Some('/') => inside = Inside::LineComment,
                '/' if next == Some('*') => {
                    inside = Inside::BlockComment;
                    index += 1;
                }
                '`' => stack.push(Context::Template),
                '{' => *stack.last_mut().expect("never empty") = Context::Code(depth + 1),
                '}' if depth > 0 => {
                    *stack.last_mut().expect("never empty") = Context::Code(depth - 1)
                }
                '}' if stack.len() > 1 => {
                    // Closes a substitution: back in its template.
                    stack.pop();
                }
                _ => {}
            },
        }
        index += 1;
    }
    lines.push(started && at_top(&stack, inside));
    lines
}

/// One line, or `None` to keep it as it is.
fn format_line(line: &str) -> Option<String> {
    let indent = line.len() - line.trim_start().len();
    let code = line.trim();
    if code.is_empty() || code.starts_with("//") || code.starts_with("/*") || code.starts_with('*')
    {
        return None;
    }
    let items = Parser {
        tokens: tokenize(code)?,
        position: 0,
    }
    .line()?;
    let mut printer = Printer {
        out: String::new(),
        column: indent,
    };
    printer.sequence(&items, indent, 0);
    let printed = format!("{}{}", &line[..indent], printer.out);
    // Only whitespace and trailing commas may change.
    (squash(&printed) == squash(line)).then_some(printed)
}

/// The text without whitespace and without commas before a closing bracket.
fn squash(text: &str) -> String {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let mut out = String::with_capacity(compact.len());
    let chars: Vec<char> = compact.chars().collect();
    for (index, c) in chars.iter().enumerate() {
        if *c == ',' && matches!(chars.get(index + 1), Some(')' | ']' | '}')) {
            continue;
        }
        out.push(*c);
    }
    out
}

#[derive(Debug, Clone)]
enum Token {
    /// Text that is never broken, and whether whitespace came before it.
    Atom(String, bool),
    Open(char, bool),
    Close(char),
    Comma,
    Semicolon,
}

fn tokenize(code: &str) -> Option<Vec<Token>> {
    let chars: Vec<char> = code.chars().collect();
    let mut tokens = Vec::new();
    let mut index = 0;
    let mut space = false;
    while index < chars.len() {
        let c = chars[index];
        match c {
            ' ' | '\t' => {
                space = true;
                index += 1;
                continue;
            }
            '(' | '[' | '{' => tokens.push(Token::Open(c, space)),
            ')' | ']' | '}' => tokens.push(Token::Close(c)),
            ',' => tokens.push(Token::Comma),
            ';' => tokens.push(Token::Semicolon),
            '"' | '\'' | '`' => {
                let end = string_end(&chars, index)?;
                let text: String = chars[index..end].iter().collect();
                if text.contains('\n') {
                    return None;
                }
                tokens.push(Token::Atom(text, space));
                space = false;
                index = end;
                continue;
            }
            '/' if matches!(chars.get(index + 1), Some('/' | '*')) => return None,
            _ => {
                let start = index;
                while index < chars.len()
                    && !matches!(
                        chars[index],
                        ' ' | '\t'
                            | '('
                            | '['
                            | '{'
                            | ')'
                            | ']'
                            | '}'
                            | ','
                            | ';'
                            | '"'
                            | '\''
                            | '`'
                    )
                {
                    if chars[index] == '/' && matches!(chars.get(index + 1), Some('/' | '*')) {
                        return None;
                    }
                    index += 1;
                }
                tokens.push(Token::Atom(chars[start..index].iter().collect(), space));
                space = false;
                continue;
            }
        }
        space = false;
        index += 1;
    }
    Some(tokens)
}

/// Index just past the string or template literal starting at `start`.
fn string_end(chars: &[char], start: usize) -> Option<usize> {
    let quote = chars[start];
    let mut index = start + 1;
    while index < chars.len() {
        match chars[index] {
            '\\' => index += 2,
            c if c == quote => return Some(index + 1),
            '$' if quote == '`' && chars.get(index + 1) == Some(&'{') => {
                // A substitution: up to its closing brace, past nested strings.
                let mut depth = 0;
                index += 1;
                loop {
                    match chars.get(index)? {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        '"' | '\'' | '`' => {
                            index = string_end(chars, index)?;
                            continue;
                        }
                        _ => {}
                    }
                    index += 1;
                }
                index += 1;
            }
            _ => index += 1,
        }
    }
    None
}

/// A part of a line.
#[derive(Debug, Clone)]
enum Node {
    Atom(String, bool),
    Group(Group),
}

#[derive(Debug, Clone)]
struct Group {
    open: char,
    space_before: bool,
    /// Items separated by commas, or statements for a block.
    items: Vec<Vec<Node>>,
    block: bool,
}

impl Group {
    fn close(&self) -> char {
        match self.open {
            '(' => ')',
            '[' => ']',
            _ => '}',
        }
    }
}

struct Parser {
    tokens: Vec<Token>,
    position: usize,
}

impl Parser {
    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.position).cloned();
        self.position += 1;
        token
    }

    /// The whole line: atoms, groups, and a final `;`.
    fn line(&mut self) -> Option<Vec<Node>> {
        let mut sequence = Vec::new();
        while let Some(token) = self.next() {
            match token {
                Token::Atom(text, space) => sequence.push(Node::Atom(text, space)),
                Token::Open(open, space) => sequence.push(Node::Group(self.group(open, space)?)),
                Token::Semicolon => sequence.push(Node::Atom(";".into(), false)),
                Token::Close(_) | Token::Comma => return None,
            }
        }
        Some(sequence)
    }

    fn group(&mut self, open: char, space_before: bool) -> Option<Group> {
        let close = match open {
            '(' => ')',
            '[' => ']',
            _ => '}',
        };
        let mut items = Vec::new();
        let mut item = Vec::new();
        let (mut commas, mut semicolons) = (false, false);
        loop {
            match self.next()? {
                Token::Close(c) if c == close => break,
                Token::Close(_) => return None,
                Token::Comma => {
                    commas = true;
                    if item.is_empty() {
                        return None;
                    }
                    items.push(std::mem::take(&mut item));
                }
                Token::Semicolon => {
                    semicolons = true;
                    if open != '{' || item.is_empty() {
                        return None;
                    }
                    items.push(std::mem::take(&mut item));
                }
                Token::Atom(text, space) => item.push(Node::Atom(text, space)),
                Token::Open(inner, space) => item.push(Node::Group(self.group(inner, space)?)),
            }
        }
        if commas && semicolons || semicolons && !item.is_empty() {
            return None;
        }
        if !item.is_empty() {
            items.push(item);
        }
        Some(Group {
            open,
            space_before,
            items,
            block: semicolons,
        })
    }
}

struct Printer {
    out: String,
    column: usize,
}

impl Printer {
    fn push(&mut self, text: &str) {
        self.out.push_str(text);
        self.column += text.chars().count();
    }

    fn newline(&mut self, indent: usize) {
        self.out.push('\n');
        self.out.push_str(&" ".repeat(indent));
        self.column = indent;
    }

    /// Print a sequence at `indent`; `tail` is the width of what must
    /// follow it before the next place a line can break.
    fn sequence(&mut self, nodes: &[Node], indent: usize, tail: usize) {
        for (index, node) in nodes.iter().enumerate() {
            if index > 0 && spaced(node) {
                self.push(" ");
            }
            match node {
                Node::Atom(text, _) => self.push(text),
                Node::Group(group) if !breakable(nodes, index) => self.push(&flat_group(group)),
                Node::Group(group) => {
                    let after = width_to_break(nodes, index + 1, tail);
                    self.group(group, indent, after);
                }
            }
        }
    }

    fn group(&mut self, group: &Group, indent: usize, tail: usize) {
        if group.items.is_empty() {
            self.push(&format!("{}{}", group.open, group.close()));
            return;
        }
        if group.block {
            self.push("{");
            for statement in &group.items {
                self.newline(indent + 2);
                self.sequence(statement, indent + 2, 1);
                self.push(";");
            }
            self.newline(indent);
            self.push("}");
            return;
        }
        let flat = group_flat_width(group);
        if self.column.saturating_add(flat).saturating_add(tail) <= WIDTH {
            self.push(&flat_group(group));
            return;
        }
        if self.hug(group, indent) {
            return;
        }
        self.break_items(group, indent);
    }

    /// A call's last argument hugs the parentheses, the others staying
    /// flat: a function, object or array breaks inside its brackets, and an
    /// arrow function's expression body goes on the next line.
    fn hug(&mut self, group: &Group, indent: usize) -> bool {
        if group.open != '(' {
            return false;
        }
        let (last, others) = group.items.split_last().expect("not empty");
        let arrow = last
            .iter()
            .position(|node| matches!(node, Node::Atom(text, _) if text == "=>"));
        let mut first_line = self.column + 1;
        for item in others {
            first_line = first_line
                .saturating_add(flat_width(item))
                .saturating_add(2);
        }
        match (last.split_last(), arrow) {
            // `(file) => { ... }`, `{ ... }`, `[ ... ]`
            (Some((Node::Group(inner), head)), _)
                if inner.open != '(' && (arrow.is_some() || head.is_empty()) =>
            {
                let space = !head.is_empty() && inner.space_before;
                first_line = first_line
                    .saturating_add(flat_width(head))
                    .saturating_add(usize::from(space) + 1);
                if first_line > WIDTH {
                    return false;
                }
                self.push("(");
                self.flat_items(others);
                self.push(&flat_sequence(head));
                if space {
                    self.push(" ");
                }
                if inner.block {
                    self.group(inner, indent, 0);
                } else {
                    self.break_items(inner, indent);
                }
                self.push(")");
                true
            }
            // `(file) =>` then the body on its own line.
            (_, Some(at)) if at + 1 < last.len() => {
                let (head, body) = last.split_at(at + 1);
                first_line = first_line.saturating_add(flat_width(head));
                if first_line > WIDTH {
                    return false;
                }
                self.push("(");
                self.flat_items(others);
                self.push(&flat_sequence(head));
                self.newline(indent + 2);
                self.sequence(body, indent + 2, 1);
                self.push(",");
                self.newline(indent);
                self.push(")");
                true
            }
            _ => false,
        }
    }

    fn flat_items(&mut self, items: &[Vec<Node>]) {
        for item in items {
            self.push(&flat_sequence(item));
            self.push(", ");
        }
    }

    fn break_items(&mut self, group: &Group, indent: usize) {
        self.push(&group.open.to_string());
        for item in &group.items {
            self.newline(indent + 2);
            self.sequence(item, indent + 2, 1);
            self.push(",");
        }
        self.newline(indent);
        self.push(&group.close().to_string());
    }
}

/// Whether the group at `index` may break: not a member access (`a["b"]`).
fn breakable(nodes: &[Node], index: usize) -> bool {
    match &nodes[index] {
        Node::Group(group) => {
            !(group.open == '[' && index > 0 && !group.space_before) && !group.items.is_empty()
        }
        Node::Atom(..) => false,
    }
}

/// The width from `start` to the first place a line can break: the
/// opening bracket of a group that may break, or `tail` past the end.
fn width_to_break(nodes: &[Node], start: usize, tail: usize) -> usize {
    let mut width = 0usize;
    for index in start..nodes.len() {
        let node = &nodes[index];
        if spaced(node) {
            width += 1;
        }
        match node {
            Node::Atom(text, _) => width += text.chars().count(),
            Node::Group(_) if breakable(nodes, index) => return width + 1,
            Node::Group(group) => width = width.saturating_add(group_flat_width(group)),
        }
    }
    width.saturating_add(tail)
}

/// Whether a space goes before this node in the sequence.
fn spaced(node: &Node) -> bool {
    match node {
        Node::Atom(_, space) => *space,
        Node::Group(group) => group.space_before,
    }
}

fn flat_width(nodes: &[Node]) -> usize {
    let mut width = 0usize;
    for (index, node) in nodes.iter().enumerate() {
        if index > 0 && spaced(node) {
            width += 1;
        }
        width = width.saturating_add(match node {
            Node::Atom(text, _) => text.chars().count(),
            Node::Group(group) => group_flat_width(group),
        });
    }
    width
}

fn group_flat_width(group: &Group) -> usize {
    if group.block {
        return usize::MAX / 4;
    }
    flat_group(group).chars().count()
}

fn flat_sequence(nodes: &[Node]) -> String {
    let mut out = String::new();
    for (index, node) in nodes.iter().enumerate() {
        if index > 0 && spaced(node) {
            out.push(' ');
        }
        match node {
            Node::Atom(text, _) => out.push_str(text),
            Node::Group(group) => out.push_str(&flat_group(group)),
        }
    }
    out
}

fn flat_group(group: &Group) -> String {
    let items: Vec<String> = group.items.iter().map(|item| flat_sequence(item)).collect();
    if group.open == '{' && !items.is_empty() {
        format!("{{ {} }}", items.join(", "))
    } else {
        format!("{}{}{}", group.open, items.join(", "), group.close())
    }
}

#[cfg(test)]
mod tests {
    use super::format;

    #[test]
    fn short_lines_stay() {
        let line = "  await step(\"Rules\", forEachFile(astGrep(configRules)));\n";
        assert_eq!(format(line), line);
        let line = "import { forEachFile, step } from \"codemod/workflow\";\n";
        assert_eq!(format(line), line);
    }

    #[test]
    fn blocks_break_and_the_last_argument_hugs() {
        assert_eq!(
            format("  await step(\"Rewrite\", forEachFile(files, (file) => { const out = transform(parse(file)); if (typeof out === \"string\") file.write(out); }));\n"),
            "  await step(\n    \"Rewrite\",\n    forEachFile(files, (file) => {\n      const out = transform(parse(file));\n      if (typeof out === \"string\") file.write(out);\n    }),\n  );\n"
        );
    }

    #[test]
    fn long_lists_go_one_item_per_line() {
        assert_eq!(
            format("const files = { include: [\"**/*.cjs\", \"**/*.cts\", \"**/*.js\", \"**/*.jsx\", \"**/*.mjs\", \"**/*.mts\", \"**/*.ts\"], exclude: [\"**/node_modules/**\"], language: \"typescript\" };\n"),
            "const files = {\n  include: [\n    \"**/*.cjs\",\n    \"**/*.cts\",\n    \"**/*.js\",\n    \"**/*.jsx\",\n    \"**/*.mjs\",\n    \"**/*.mts\",\n    \"**/*.ts\",\n  ],\n  exclude: [\"**/node_modules/**\"],\n  language: \"typescript\",\n};\n"
        );
    }

    #[test]
    fn strings_templates_and_comments_are_kept() {
        let text = "  await step(`a ${\"(\"} b`, shell(\"echo (\"));\n// a (comment\nconst x = `line\nline`;\n";
        assert_eq!(format(text), text);
    }
}
