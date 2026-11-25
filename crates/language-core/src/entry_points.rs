//! Entry points: functions something outside the workspace's own code calls.
//!
//! `callers()` only returns call sites. Whether a function with no callers is
//! still used is a separate question, answered here: it is exported through
//! its package's public API, or a rule on the selector declares it (a route
//! handler, a task).

use serde::{Deserialize, Serialize};

/// A rule from `semanticAnalysis.entryPoints`: functions it matches are
/// entry points of its `kind`.
///
/// ```text
/// { files: "app/**/route.ts", exports: ["GET", "POST"], kind: "http" }
/// { decorators: ["*.route", "shared_task"], kind: "handler" }
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntryPointRule {
    /// A glob over the path relative to the workspace root. Absent: any file.
    #[serde(default)]
    pub files: Option<String>,
    /// Export names that match (`"default"` for the default export). With
    /// `decorators` too, a function must match both.
    #[serde(default)]
    pub exports: Vec<String>,
    /// Decorator names that match, without arguments (`app.route` for
    /// `@app.route("/x")`); `*` matches any run of characters.
    #[serde(default)]
    pub decorators: Vec<String>,
    pub kind: String,
}

/// Why a function is an entry point.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EntryPoint {
    /// `"export"` for the package's public API, else the rule's `kind`.
    pub kind: String,
    /// What reaches it: `@acme/ui#Button`, `GET app/api/users/[id]/route.ts`.
    pub label: String,
}

/// A function in the workspace that is called from outside its code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionEntryPoints {
    pub file_path: std::path::PathBuf,
    /// The function's range.
    pub range: crate::ByteRange,
    /// Why it is an entry point; never empty.
    pub entry_points: Vec<EntryPoint>,
}

/// What a provider knows about one function, for the declared rules.
#[derive(Debug, Clone, Default)]
pub struct FunctionEntry<'a> {
    /// The file, relative to the workspace root, with `/` separators.
    pub file: &'a str,
    /// The function's name, for labels.
    pub name: Option<&'a str>,
    /// Names the function is exported under from its file.
    pub exports: &'a [String],
    /// Its decorators, as written (`app.route("/x")`).
    pub decorators: &'a [String],
}

/// The entry points `rules` declare for `function`.
pub fn declared_entry_points(
    rules: &[EntryPointRule],
    function: &FunctionEntry,
) -> Vec<EntryPoint> {
    let mut out = Vec::new();
    for rule in rules {
        if rule.exports.is_empty() && rule.decorators.is_empty() {
            continue;
        }
        if let Some(files) = &rule.files {
            if !glob_matches(files, function.file) {
                continue;
            }
        }
        let export = if rule.exports.is_empty() {
            None
        } else {
            match function
                .exports
                .iter()
                .find(|name| rule.exports.contains(name))
            {
                Some(name) => Some(name.as_str()),
                None => continue,
            }
        };
        let decorator = if rule.decorators.is_empty() {
            None
        } else {
            let found = function
                .decorators
                .iter()
                .map(|d| decorator_name(d))
                .find(|name| {
                    rule.decorators
                        .iter()
                        .any(|pattern| wildcard_matches(pattern, name))
                });
            match found {
                Some(name) => Some(name),
                None => continue,
            }
        };
        let label = match (export, decorator) {
            (Some(export), _) => format!("{export} {}", function.file),
            (None, Some(decorator)) => format!(
                "@{decorator} {}:{}",
                function.file,
                function.name.unwrap_or("<anonymous>")
            ),
            (None, None) => unreachable!("a rule names exports or decorators"),
        };
        out.push(EntryPoint {
            kind: rule.kind.clone(),
            label,
        });
    }
    out
}

/// `app.route("/x")` → `app.route`.
fn decorator_name(text: &str) -> &str {
    text.split('(').next().unwrap_or(text).trim()
}

/// `*` matches any run of characters, everything else itself.
fn wildcard_matches(pattern: &str, text: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or_default();
    let Some(mut rest) = text.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        return rest.is_empty();
    };
    for part in middle {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

/// A path glob: `**` matches any number of directories, `*` and `?` stay
/// within one path segment. Brackets are literal (`[id]` is a Next.js
/// segment, not a character class).
pub fn glob_matches(pattern: &str, path: &str) -> bool {
    let pattern: Vec<&str> = pattern.trim_start_matches("./").split('/').collect();
    let path: Vec<&str> = path.split('/').collect();
    segments_match(&pattern, &path)
}

fn segments_match(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| segments_match(rest, &path[skip..])),
        Some((segment, rest)) => match path.split_first() {
            Some((name, path_rest)) => {
                segment_matches(segment, name) && segments_match(rest, path_rest)
            }
            None => false,
        },
    }
}

fn segment_matches(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    fn go(p: &[char], n: &[char]) -> bool {
        match p.split_first() {
            None => n.is_empty(),
            Some(('*', rest)) => (0..=n.len()).any(|skip| go(rest, &n[skip..])),
            Some(('?', rest)) => !n.is_empty() && go(rest, &n[1..]),
            Some((c, rest)) => n.first() == Some(c) && go(rest, &n[1..]),
        }
    }
    go(&pattern, &name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_keep_brackets_literal_and_cross_directories_only_with_double_stars() {
        assert!(glob_matches(
            "app/**/route.ts",
            "app/api/users/[id]/route.ts"
        ));
        assert!(glob_matches("app/**/route.ts", "app/route.ts"));
        assert!(!glob_matches("app/*/route.ts", "app/api/users/route.ts"));
        assert!(glob_matches("src/*.ts", "src/a.ts"));
    }

    #[test]
    fn a_decorator_matches_by_name_without_its_arguments() {
        let rules = [EntryPointRule {
            decorators: vec!["*.route".into()],
            kind: "handler".into(),
            ..Default::default()
        }];
        let decorators = ["app.route(\"/users\")".to_string()];
        let function = FunctionEntry {
            file: "api/views.py",
            name: Some("users"),
            decorators: &decorators,
            ..Default::default()
        };
        assert_eq!(
            declared_entry_points(&rules, &function),
            [EntryPoint {
                kind: "handler".into(),
                label: "@app.route api/views.py:users".into()
            }]
        );
    }
}
