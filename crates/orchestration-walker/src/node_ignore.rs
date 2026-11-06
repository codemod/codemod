//! A port of the `ignore` npm package, version 5.3.1 (`node-ignore`, the
//! matcher `packages/orchestration/src/execution/files.ts` uses), exact by
//! construction: every rule is turned into the same JavaScript `RegExp`
//! source by the same chain of replacements (`REPLACERS` in its `index.js`),
//! and every regular expression, both the replacements' and the rules', runs
//! on `regress`, an ECMAScript engine, in non-unicode mode over UTF-16 code
//! units, as V8 and QuickJS run them. Rules are case-insensitive (the
//! package's default `ignorecase: true`).
//!
//! What is ported, by `index.js` name: `checkPattern`, `splitPattern`,
//! `createRule`, `makeRegex` (all of `REPLACERS`, `sanitizeRange`,
//! `cleanRangeBackSlash`), `checkPath` (POSIX), `_testOne` with
//! `checkUnignored` (the last matching rule wins), and `_t` (a path is
//! ignored when any parent directory is). Regex construction errors make the
//! whole `add()` fail, as `new RegExp` throwing does.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use regress::{Flags, Match as RegexMatch, Regex};

/// `ignore().test(path)` as the walker reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Match {
    /// `{ ignored: false, unignored: false }`
    None,
    /// `{ ignored: true }`
    Ignore,
    /// `{ unignored: true }`
    Whitelist,
}

/// `new RegExp(source, "i")` threw: the rule set cannot be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternError {
    pub pattern: String,
    pub source: String,
    pub message: String,
}

impl std::fmt::Display for PatternError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Invalid regular expression: /{}/i: {} (pattern '{}')",
            self.source, self.message, self.pattern
        )
    }
}

/// `checkPath` refused the path (`TypeError`/`RangeError` in JS).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathError(pub String);

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn js(source: &str) -> Regex {
    Regex::new(source).unwrap_or_else(|error| panic!("built-in regex /{source}/: {error}"))
}

/// The fixed regular expressions of `index.js`, compiled once.
struct Statics {
    blank_line: Regex,
    invalid_trailing_backslash: Regex,
    leading_escaped_exclamation: Regex,
    leading_escaped_hash: Regex,
    split_crlf: Regex,
    slash_not_at_end: Regex,
    escaped_star: Regex,
    range: Regex,
    ends_with_slash: Regex,
    replacers: Vec<Regex>,
}

fn statics() -> &'static Statics {
    static STATICS: OnceLock<Statics> = OnceLock::new();
    STATICS.get_or_init(|| Statics {
        blank_line: js(r"^\s+$"),
        invalid_trailing_backslash: js(r"(?:[^\\]|^)\\$"),
        leading_escaped_exclamation: js(r"^\\!"),
        leading_escaped_hash: js(r"^\\#"),
        split_crlf: js(r"\r?\n"),
        slash_not_at_end: js(r"\/(?!$)"),
        escaped_star: js(r"\\\*"),
        range: js(r"([0-z])-([0-z])"),
        ends_with_slash: js(r"\/$"),
        replacers: [
            r"^\uFEFF",
            r"\\?\s+$",
            r"\\\s",
            r"[\\$.|*+(){^]",
            r"(?!\\)\?",
            r"^\/",
            r"\/",
            r"^\^*\\\*\\\*\\\/",
            r"^(?=[^^])",
            r"\\\/\\\*\\\*(?=\\\/|$)",
            r"(^|[^\\]+)(\\\*)+(?=.+)",
            r"\\\\\\(?=[$.|*+(){^])",
            r"\\\\",
            r"(\\)?\[([^\]/]*?)(\\*)($|\])",
            r"(?:[^*])$",
            r"(\^|\\\/)?\\\*$",
        ]
        .into_iter()
        .map(js)
        .collect(),
    })
}

fn test(regex: &Regex, text: &str) -> bool {
    regex.find(text).is_some()
}

fn group<'t>(text: &'t str, found: &RegexMatch, index: usize) -> Option<&'t str> {
    found.group(index).map(|range| &text[range])
}

/// `text.replace(regex, callback)`: the first match, or every match when
/// `global` (the JS `g` flag). None of the global expressions here can match
/// the empty string.
fn replace(
    text: &str,
    regex: &Regex,
    global: bool,
    mut callback: impl FnMut(&str, &RegexMatch) -> String,
) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let mut last = 0;
    for found in regex.find_iter(text) {
        out.push_str(&text[last..found.start()]);
        out.push_str(&callback(text, &found));
        last = found.end();
        if !global {
            break;
        }
    }
    out.push_str(&text[last..]);
    out
}

/// `sanitizeRange`: drop out-of-order ranges (`z-a`), fatal in a JS regex.
fn sanitize_range(range: &str) -> String {
    replace(range, &statics().range, true, |text, found| {
        let from = group(text, found, 1).unwrap_or_default();
        let to = group(text, found, 2).unwrap_or_default();
        // `charCodeAt(0)`: both are single ASCII characters (`[0-z]`).
        if from.encode_utf16().next() <= to.encode_utf16().next() {
            text[found.range()].to_string()
        } else {
            String::new()
        }
    })
}

/// `cleanRangeBackSlash`: keep an even number of backslashes.
fn clean_range_back_slash(slashes: &str) -> &str {
    let length = slashes.len();
    &slashes[..length - length % 2]
}

/// `makeRegex(pattern)`: the `RegExp` source of one rule (without `!`).
pub fn make_source(pattern: &str) -> String {
    let s = statics();
    let r = &s.replacers;
    // `\uFEFF` (BOM) at the start.
    let mut source = replace(pattern, &r[0], false, |_, _| String::new());
    // Trailing spaces, unless escaped.
    source = replace(&source, &r[1], false, |text, found| {
        if text[found.range()].starts_with('\\') {
            " ".to_string()
        } else {
            String::new()
        }
    });
    // `\ ` -> ` `.
    source = replace(&source, &r[2], true, |_, _| " ".to_string());
    // Escape regex metacharacters.
    source = replace(&source, &r[3], true, |text, found| {
        format!("\\{}", &text[found.range()])
    });
    // `?`.
    source = replace(&source, &r[4], true, |_, _| "[^/]".to_string());
    // Leading slash.
    source = replace(&source, &r[5], false, |_, _| "^".to_string());
    // Every other slash.
    source = replace(&source, &r[6], true, |_, _| "\\/".to_string());
    // Leading `**/`.
    source = replace(&source, &r[7], false, |_, _| "^(?:.*\\/)?".to_string());
    // Starting: `this` is the pattern makeRegex was called with.
    let anchored = test(&s.slash_not_at_end, pattern);
    source = replace(&source, &r[8], false, |_, _| {
        if anchored { "^" } else { "(?:^|\\/)" }.to_string()
    });
    // `/**/` and trailing `/**`.
    source = replace(&source, &r[9], true, |text, found| {
        // `index + 6 < str.length`: the 6-unit ASCII match is not at the end.
        if found.end() < text.len() {
            "(?:\\/[^\\/]+)*".to_string()
        } else {
            "\\/.+".to_string()
        }
    });
    // Intermediate wildcards.
    source = replace(&source, &r[10], true, |text, found| {
        let p1 = group(text, found, 1).unwrap_or_default();
        // A repeated group captures its last iteration only.
        let p2 = group(text, found, 2).unwrap_or_default();
        let unescaped = replace(p2, &s.escaped_star, true, |_, _| "[^\\/]*".to_string());
        format!("{p1}{unescaped}")
    });
    // Unescape escaped metacharacters.
    source = replace(&source, &r[11], true, |_, _| "\\".to_string());
    source = replace(&source, &r[12], true, |_, _| "\\".to_string());
    // Range notation.
    source = replace(&source, &r[13], true, |text, found| {
        let lead_escape = group(text, found, 1);
        let range = group(text, found, 2).unwrap_or_default();
        let end_escape = group(text, found, 3).unwrap_or_default();
        let close = group(text, found, 4).unwrap_or_default();
        if lead_escape == Some("\\") {
            format!("\\[{range}{}{close}", clean_range_back_slash(end_escape))
        } else if close == "]" {
            if end_escape.len() % 2 == 0 {
                format!("[{}{end_escape}]", sanitize_range(range))
            } else {
                "[]".to_string()
            }
        } else {
            "[]".to_string()
        }
    });
    // Ending.
    source = replace(&source, &r[14], false, |text, found| {
        let matched = &text[found.range()];
        if test(&s.ends_with_slash, matched) {
            format!("{matched}$")
        } else {
            format!("{matched}(?=$|\\/$)")
        }
    });
    // Trailing wildcard.
    replace(&source, &r[15], false, |text, found| {
        let prefix = match group(text, found, 1) {
            Some(p1) if !p1.is_empty() => format!("{p1}[^/]+"),
            _ => "[^/]*".to_string(),
        };
        format!("{prefix}(?=$|\\/$)")
    })
}

/// `checkPattern`.
fn check_pattern(pattern: &str) -> bool {
    let s = statics();
    !pattern.is_empty()
        && !test(&s.blank_line, pattern)
        && !test(&s.invalid_trailing_backslash, pattern)
        && !pattern.starts_with('#')
}

struct Rule {
    negative: bool,
    regex: Arc<Compiled>,
}

/// One rule's `RegExp`: always on `regress`, and on the `regex` crate too
/// when [`fast_source`] proves both engines agree on ASCII paths.
struct Compiled {
    js: Regex,
    fast: Option<regex::Regex>,
}

impl Compiled {
    fn is_match(&self, path: &str, units: Option<&[u16]>) -> bool {
        match (units, &self.fast) {
            (None, Some(fast)) => fast.is_match(path),
            // ASCII text is the same sequence as code points and UTF-16 units.
            (None, None) => self.js.find(path).is_some(),
            (Some(units), _) => self.js.find_from_ucs2(units, 0).next().is_some(),
        }
    }
}

/// The rule's source for the `regex` crate, when the rule's text is plain
/// enough that the generated source uses only constructs both engines read
/// alike on ASCII input: rule text that is ASCII without `\`, `[`, `]`,
/// `{`, `}` (so every escape in the source is a generated punctuation escape
/// and there is no user character class), `.` rewritten to JS's "anything
/// but a line terminator", and the trailing `(?=$|\/$)` lookahead (the only
/// one `makeRegex` emits, always last) as a group, equivalent for a yes/no
/// search. Case-insensitivity on ASCII text is ASCII folding in both. `None`
/// keeps the rule on `regress` alone.
fn fast_source(body: &str, source: &str) -> Option<String> {
    if !body.is_ascii() || body.contains(['\\', '[', ']', '{', '}']) {
        return None;
    }
    let source = match source.strip_suffix(r"(?=$|\/$)") {
        Some(rest) => format!(r"{rest}(?:$|\/$)"),
        None => source.to_string(),
    };
    if source.contains("(?=") || source.contains("(?!") || source.contains("(?<") {
        return None;
    }
    let mut out = String::from("(?i)");
    let mut chars = source.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                out.push('\\');
                out.push(chars.next()?);
            }
            '.' => out.push_str(r"[^\n\r\x{2028}\x{2029}]"),
            other => out.push(other),
        }
    }
    Some(out)
}

/// Compiled rules by source, shared by every matcher one selection builds.
#[derive(Default)]
pub struct RuleCache {
    compiled: HashMap<String, Result<Arc<Compiled>, String>>,
}

impl RuleCache {
    fn compile(&mut self, body: &str, source: String) -> Result<Arc<Compiled>, (String, String)> {
        let entry = self.compiled.entry(source.clone()).or_insert_with(|| {
            let flags = Flags {
                icase: true,
                ..Flags::default()
            };
            let js = Regex::from_unicode(source.encode_utf16().map(u32::from), flags)
                .map_err(|error| error.to_string())?;
            let fast = fast_source(body, &source).and_then(|fast| regex::Regex::new(&fast).ok());
            Ok(Arc::new(Compiled { js, fast }))
        });
        entry.clone().map_err(|message| (source, message))
    }
}

/// `createRule`.
fn create_rule(pattern: &str, cache: &mut RuleCache) -> Result<Rule, PatternError> {
    let s = statics();
    let (negative, body) = match pattern.strip_prefix('!') {
        Some(rest) => (true, rest),
        None => (false, pattern),
    };
    let body = replace(body, &s.leading_escaped_exclamation, false, |_, _| {
        "!".to_string()
    });
    let body = replace(&body, &s.leading_escaped_hash, false, |_, _| {
        "#".to_string()
    });
    let source = make_source(&body);
    let regex = cache
        .compile(&body, source)
        .map_err(|(source, message)| PatternError {
            pattern: pattern.to_string(),
            source,
            message,
        })?;
    Ok(Rule { negative, regex })
}

/// One `ignore()` instance: its rules and its `_testCache` (shared by the
/// walker's threads; results are pure, so races only repeat work).
pub struct Ignore {
    rules: Vec<Rule>,
    cache: Mutex<HashMap<String, Match>>,
}

impl Ignore {
    /// `ignore().add(patterns)` with an array: each element is one pattern.
    pub fn from_patterns<'a>(
        patterns: impl IntoIterator<Item = &'a str>,
        cache: &mut RuleCache,
    ) -> Result<Self, PatternError> {
        let mut rules = Vec::new();
        for pattern in patterns {
            if check_pattern(pattern) {
                rules.push(create_rule(pattern, cache)?);
            }
        }
        Ok(Self {
            rules,
            cache: Mutex::new(HashMap::new()),
        })
    }

    /// `ignore().add(text)` with a string: one pattern per line (`\r?\n`).
    pub fn from_text(text: &str, cache: &mut RuleCache) -> Result<Self, PatternError> {
        let split = &statics().split_crlf;
        let mut lines = Vec::new();
        let mut last = 0;
        for found in split.find_iter(text) {
            lines.push(&text[last..found.start()]);
            last = found.end();
        }
        lines.push(&text[last..]);
        Self::from_patterns(lines, cache)
    }

    /// `test(path)`: `checkPath`, then `_t`.
    pub fn test(&self, path: &str) -> Result<Match, PathError> {
        check_path(path)?;
        let bytes = path.as_bytes();
        // `_t`: the first parent directory (`a/`, `a/b/`, ...) that is
        // ignored makes the path ignored; otherwise the path's own rules decide.
        for (index, byte) in bytes.iter().enumerate() {
            if *byte != b'/' || index + 1 == bytes.len() {
                continue;
            }
            if self.cached(&path[..=index]) == Match::Ignore {
                return Ok(Match::Ignore);
            }
        }
        Ok(if path.ends_with('/') {
            self.cached(path)
        } else {
            self.test_one(path)
        })
    }

    /// `_testOne` of a directory prefix, memoized like `_testCache`. Only
    /// reached when no shorter prefix is ignored, so this is also its `_t`.
    fn cached(&self, dir: &str) -> Match {
        fn lock(cache: &Mutex<HashMap<String, Match>>) -> MutexGuard<'_, HashMap<String, Match>> {
            cache
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        }
        if let Some(result) = lock(&self.cache).get(dir) {
            return *result;
        }
        let result = self.test_one(dir);
        lock(&self.cache).insert(dir.to_string(), result);
        result
    }

    /// `_testOne(path, checkUnignored = true)`: the last matching rule wins.
    fn test_one(&self, path: &str) -> Match {
        if self.rules.is_empty() {
            return Match::None;
        }
        let units: Option<Vec<u16>> = (!path.is_ascii()).then(|| path.encode_utf16().collect());
        for rule in self.rules.iter().rev() {
            if rule.regex.is_match(path, units.as_deref()) {
                return if rule.negative {
                    Match::Whitelist
                } else {
                    Match::Ignore
                };
            }
        }
        Match::None
    }
}

/// `checkPath` (POSIX): non-empty, and not `/x`, `./x`, `../x`, `.`, `..`, `...`.
fn check_path(path: &str) -> Result<(), PathError> {
    if path.is_empty() {
        return Err(PathError("path must not be empty".to_string()));
    }
    let dots = path.bytes().take_while(|byte| *byte == b'.').count();
    if path[dots..].starts_with('/') || dots == path.len() {
        return Err(PathError(format!(
            "path should be a `path.relative()`d string, but got \"{path}\""
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(pattern: &str) -> String {
        make_source(pattern)
    }

    #[test]
    fn regex_sources_match_node_ignore() {
        // Expected values printed by node-ignore 5.3.1
        // (`ignore().add(p)._rules[0].regex.source`).
        let cases = [
            ("node_modules", r"(?:^|\/)node_modules(?=$|\/$)"),
            ("*.ts", r"(?:^|\/)[^\/]*\.ts(?=$|\/$)"),
            ("/top-only.ts", r"^top-only\.ts(?=$|\/$)"),
            ("build/", r"(?:^|\/)build\/$"),
            ("a/**/b", r"^a(?:\/[^\/]+)*\/b(?=$|\/$)"),
            ("**/foo", r"^(?:.*\/)?foo(?=$|\/$)"),
            ("foo/**", r"^foo\/.+(?=$|\/$)"),
            ("foo/*", r"^foo\/[^/]+(?=$|\/$)"),
            ("a?c", r"(?:^|\/)a[^\/]c(?=$|\/$)"),
            ("[a-z].js", r"(?:^|\/)[a-z]\.js(?=$|\/$)"),
            ("[z-a].js", r"(?:^|\/)[]\.js(?=$|\/$)"),
            ("abc*", r"(?:^|\/)abc[^/]*(?=$|\/$)"),
            ("#hash", r"(?:^|\/)#hash(?=$|\/$)"),
            ("a\\ ", r"(?:^|\/)a (?=$|\/$)"),
            ("x  ", r"(?:^|\/)x(?=$|\/$)"),
            ("**", r"(?:^|\/)[^\/]*[^/]*(?=$|\/$)"),
            ("*", r"(?:^|\/)[^/]*(?=$|\/$)"),
            ("a/**/", r"^a(?:\/[^\/]+)*\/$"),
            ("[!a]y", r"(?:^|\/)[!a]y(?=$|\/$)"),
            ("foo[bar/baz", r"^foo[bar\/baz(?=$|\/$)"),
        ];
        for (pattern, expected) in cases {
            assert_eq!(source(pattern), expected, "{pattern}");
        }
    }

    #[test]
    fn the_fast_path_agrees_with_regress_on_ascii_paths() {
        let patterns = [
            "node_modules",
            "*.ts",
            "*.TS",
            "/top.ts",
            "build/",
            "a/**/b",
            "**/foo",
            "foo/**",
            "foo/*",
            "a?c",
            "abc*",
            "*",
            "**",
            "a/**/",
            "*.{ts,js}",
            "x.y+z",
            "a|b",
            "(a)",
            "^a$",
            "dir/*.log",
            "**/deep/**/x.ts",
            "abc.*/",
            "spaced.ts   ",
            ".hidden",
            ".*",
            "*/",
            "?",
            "??",
            "a**b",
            "**/*.ts/",
            "*.min.*",
        ];
        let paths = [
            "a",
            "A",
            "a.ts",
            "A.TS",
            "src/a.ts",
            "src/",
            "node_modules/",
            "x/node_modules/y",
            "top.ts",
            "x/top.ts",
            "build",
            "build/",
            "x/build/",
            "a/b",
            "a/x/y/b",
            "foo/",
            "foo/x",
            "foo/x/",
            "foo/x/y",
            "abc",
            "abcd/",
            "a.b/c",
            "x.y+z",
            "a|b",
            "(a)",
            "^a$",
            "dir/x.log",
            "deep/1/x.ts",
            "q/deep/x.ts",
            "abc.d/",
            "spaced.ts",
            ".hidden/",
            ".env",
            "new\nline/foo",
            "cr\rdir/x.ts",
            "a\r",
            "abc.min.js",
            "x/y.ts/",
        ];
        let mut cache = RuleCache::default();
        let mut fast_rules = 0;
        for pattern in patterns {
            let source = make_source(pattern);
            let compiled = cache.compile(pattern, source.clone()).expect("compiles");
            let Some(fast) = &compiled.fast else { continue };
            fast_rules += 1;
            for path in paths {
                assert_eq!(
                    fast.is_match(path),
                    compiled.js.find(path).is_some(),
                    "pattern {pattern:?} (/{source}/i) on {path:?}"
                );
            }
        }
        assert!(
            fast_rules >= patterns.len() - 2,
            "most rules take the fast path"
        );
    }

    #[test]
    fn a_rule_js_cannot_compile_fails_the_whole_set() {
        // `new RegExp("^foo[bar\\/baz(?=$|\\/$)", "i")` throws in JS.
        let mut cache = RuleCache::default();
        assert!(Ignore::from_text("ok.ts\nfoo[bar/baz\n", &mut cache).is_err());
        assert!(Ignore::from_text("ok.ts\n[z-a].js\n", &mut cache).is_ok());
    }

    #[test]
    fn last_rule_wins_and_parents_are_checked() {
        let mut cache = RuleCache::default();
        let ignore = Ignore::from_text("foo/\n!foo/keep.ts\n*.log\n!important.log\n", &mut cache)
            .expect("rules");
        assert_eq!(ignore.test("foo/keep.ts"), Ok(Match::Ignore));
        assert_eq!(ignore.test("foo/"), Ok(Match::Ignore));
        assert_eq!(ignore.test("a.log"), Ok(Match::Ignore));
        assert_eq!(ignore.test("A.LOG"), Ok(Match::Ignore));
        assert_eq!(ignore.test("important.log"), Ok(Match::Whitelist));
        assert_eq!(ignore.test("src/a.ts"), Ok(Match::None));
        assert!(ignore.test("...").is_err());
        assert!(ignore.test(".../").is_err());
        assert!(ignore.test("").is_err());
        assert_eq!(ignore.test(".a"), Ok(Match::None));
    }
}
