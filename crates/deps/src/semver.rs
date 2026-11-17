//! Version ranges per ecosystem: npm ranges follow node-semver, Cargo
//! requirements follow Cargo's rules (`"1.2"` means `^1.2`), and Python
//! versions follow PEP 440, with Poetry's `^`, `~` and `||` read too.
//! Maven versions order as Maven 3's `ComparableVersion` does, with Maven's
//! range syntax and Gradle's bare and `1.+` versions.

use std::cmp::Ordering;

use crate::model::Ecosystem;

/// Whether `version` satisfies `range`; `None` when either cannot be read
/// (`workspace:`, `git:`, `file:` and other non-registry ranges).
pub fn satisfies(ecosystem: Ecosystem, range: &str, version: &str) -> Option<bool> {
    match ecosystem {
        Ecosystem::Npm => {
            // Any protocol but `npm:` (`file:`, `link:`, `git+https:` ...)
            // names no registry range, even when it ends in `@^1`.
            if range.contains(':') && !range.starts_with("npm:") {
                return None;
            }
            let range = range.strip_prefix("npm:").unwrap_or(range);
            // An alias ("npm:other@^1") carries its range after the last '@'.
            let range = match range.rfind('@') {
                Some(at) if at > 0 => &range[at + 1..],
                _ => range,
            };
            if range.contains(':') {
                return None;
            }
            let range = node_semver::Range::parse(range).ok()?;
            Some(range.satisfies(&node_semver::Version::parse(version).ok()?))
        }
        Ecosystem::Cargo => {
            let range = semver::VersionReq::parse(range).ok()?;
            Some(range.matches(&semver::Version::parse(version).ok()?))
        }
        Ecosystem::Pypi => pypi::satisfies(range, version),
        Ecosystem::Maven => maven::satisfies(range, version),
    }
}

pub fn is_prerelease(ecosystem: Ecosystem, version: &str) -> bool {
    match ecosystem {
        Ecosystem::Npm => node_semver::Version::parse(version).map_or(true, |v| v.is_prerelease()),
        Ecosystem::Cargo => semver::Version::parse(version).map_or(true, |v| !v.pre.is_empty()),
        Ecosystem::Pypi => pypi::version(version).is_none_or(|v| v.any_prerelease()),
        Ecosystem::Maven => maven::is_prerelease(version),
    }
}

/// A total order: versions the ecosystem can read sort by its rules, and
/// any it cannot (PyPI still lists pre-PEP 440 ones) sort before them by text.
pub fn compare(ecosystem: Ecosystem, a: &str, b: &str) -> Ordering {
    fn by<T: Ord>(a: &str, b: &str, parse: impl Fn(&str) -> Option<T>) -> Ordering {
        match (parse(a), parse(b)) {
            (Some(x), Some(y)) => x.cmp(&y),
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            (None, None) => a.cmp(b),
        }
    }
    match ecosystem {
        Ecosystem::Npm => by(a, b, |v| node_semver::Version::parse(v).ok()),
        Ecosystem::Cargo => by(a, b, |v| semver::Version::parse(v).ok()),
        Ecosystem::Pypi => by(a, b, pypi::version),
        // Maven orders every string, so this is total already.
        Ecosystem::Maven => maven::compare(a, b),
    }
}

/// Whether going from `from` to `to` is a breaking change under caret rules:
/// the left-most non-zero component changes (so 0.29 to 0.30 is breaking).
pub fn is_breaking(ecosystem: Ecosystem, from: &str, to: &str) -> bool {
    fn breaking_part<T: PartialEq + Copy>(parts: &[T], zero: T) -> Vec<T> {
        let parts = &parts[..parts.len().min(3)];
        match parts.iter().position(|part| *part != zero) {
            Some(index) => parts[..=index].to_vec(),
            None => parts.to_vec(),
        }
    }
    if matches!(ecosystem, Ecosystem::Pypi) {
        // Compare release numbers, so 1.0rc1 to 1.0 is not breaking.
        // Padded to three numbers, so 0.0 to 0.0.0 is not breaking.
        let release = |version: &pep440_rs::Version| {
            let mut release = version.release().to_vec();
            release.resize(release.len().max(3), 0);
            release
        };
        if let Some((from, to)) = pypi::version(from).zip(pypi::version(to)) {
            return from.epoch() != to.epoch()
                || breaking_part(&release(&from), 0) != breaking_part(&release(&to), 0);
        }
    }
    let split = |version| -> Vec<&str> { str::split(version, ['.', '-', '+']).collect() };
    breaking_part(&split(from), "0") != breaking_part(&split(to), "0")
}

mod pypi {
    use std::str::FromStr;

    use pep440_rs::{Version, VersionSpecifier};

    pub(super) fn version(text: &str) -> Option<Version> {
        Version::from_str(text).ok()
    }

    /// A PEP 440 specifier set, or a Poetry constraint: alternatives joined
    /// by `||` or `|`, each a list of clauses that must all hold, separated by
    /// commas or, in Poetry, by spaces (`>=1.2 <2.0`).
    pub(super) fn satisfies(range: &str, version: &str) -> Option<bool> {
        let version = self::version(version)?;
        let mut any = range.trim().is_empty();
        for alternative in range.split('|').filter(|a| !a.trim().is_empty()) {
            let mut all = true;
            for clause in clauses(alternative) {
                for specifier in specifiers(&clause)? {
                    all &= VersionSpecifier::from_str(&specifier)
                        .ok()?
                        .contains(&version);
                }
            }
            any |= all;
        }
        Some(any)
    }

    /// An alternative's clauses, each with its spaces removed: a word that
    /// is only an operator (`>= 1.2`) joins the word after it.
    fn clauses(alternative: &str) -> Vec<String> {
        let mut clauses: Vec<String> = Vec::new();
        let mut operator = String::new();
        for word in alternative
            .split([',', ' ', '\t'])
            .filter(|w| !w.is_empty())
        {
            if word
                .chars()
                .all(|c| matches!(c, '<' | '>' | '=' | '!' | '~' | '^'))
            {
                operator.push_str(word);
            } else {
                clauses.push(std::mem::take(&mut operator) + word);
            }
        }
        if !operator.is_empty() {
            clauses.push(operator);
        }
        clauses
    }

    /// One clause as PEP 440 specifiers: Poetry's `^1.2` is `>=1.2,<2`,
    /// `~1.2` is `>=1.2,<1.3`, and a bare `1.2` is `==1.2`.
    fn specifiers(clause: &str) -> Option<Vec<String>> {
        if clause.is_empty() || clause == "*" {
            return Some(Vec::new());
        }
        if let Some(lower) = clause.strip_prefix('^') {
            let release = version(lower)?.release().to_vec();
            let at = release.iter().position(|&p| p != 0);
            return between(lower, &release, at.unwrap_or(release.len() - 1));
        }
        if let Some(lower) = clause
            .strip_prefix('~')
            .filter(|rest| !rest.starts_with('='))
        {
            let release = version(lower)?.release().to_vec();
            return between(lower, &release, usize::from(release.len() > 1));
        }
        if clause.starts_with(|c: char| c.is_ascii_digit()) {
            return Some(vec![format!("=={clause}")]);
        }
        Some(vec![clause.to_string()])
    }

    /// `>=lower` and below `lower`'s release with the number at `at` raised
    /// by one, in `lower`'s epoch; `None` when that number cannot be raised.
    fn between(lower: &str, release: &[u64], at: usize) -> Option<Vec<String>> {
        let mut upper: Vec<String> = release[..at].iter().map(u64::to_string).collect();
        upper.push(release[at].checked_add(1)?.to_string());
        let epoch = match version(lower)?.epoch() {
            0 => String::new(),
            epoch => format!("{epoch}!"),
        };
        Some(vec![
            format!(">={lower}"),
            format!("<{epoch}{}", upper.join(".")),
        ])
    }
}

mod maven {
    use std::cmp::Ordering;

    /// Qualifiers in order; any other sorts after them, alphabetically.
    const QUALIFIERS: [&str; 7] = ["alpha", "beta", "milestone", "rc", "snapshot", "", "sp"];
    const RELEASE: usize = 5;

    /// A parsed version, as Maven 3's `ComparableVersion` builds it: `.`
    /// separates items, while `-` and a switch between digits and letters
    /// start a nested list.
    enum Item {
        /// Digits without leading zeros, so longer is larger.
        Number(String),
        Qualifier(String),
        List(Vec<Item>),
    }

    impl Item {
        fn is_null(&self) -> bool {
            match self {
                Item::Number(digits) => digits == "0",
                Item::Qualifier(text) => text.is_empty(),
                Item::List(items) => items.is_empty(),
            }
        }

        /// Ordering against `other`; `None` is a missing item, as in `1` against `1.0`.
        fn cmp(&self, other: Option<&Item>) -> Ordering {
            use Item::*;
            match (self, other) {
                (Number(a), None) => {
                    if a == "0" {
                        Ordering::Equal
                    } else {
                        Ordering::Greater
                    }
                }
                (Number(a), Some(Number(b))) => a.len().cmp(&b.len()).then_with(|| a.cmp(b)),
                (Number(_), Some(_)) => Ordering::Greater,
                (Qualifier(a), None) => rank(a).cmp(&rank("")),
                (Qualifier(a), Some(Qualifier(b))) => rank(a).cmp(&rank(b)),
                (Qualifier(_), Some(_)) => Ordering::Less,
                (List(items), None) => items
                    .iter()
                    .map(|item| item.cmp(None))
                    .find(|order| order.is_ne())
                    .unwrap_or(Ordering::Equal),
                (List(_), Some(Number(_))) => Ordering::Less,
                (List(_), Some(Qualifier(_))) => Ordering::Greater,
                (List(a), Some(List(b))) => lists(a, b),
            }
        }
    }

    fn lists(a: &[Item], b: &[Item]) -> Ordering {
        (0..a.len().max(b.len()))
            .map(|i| match (a.get(i), b.get(i)) {
                (Some(left), right) => left.cmp(right),
                (None, Some(right)) => right.cmp(None).reverse(),
                (None, None) => Ordering::Equal,
            })
            .find(|order| order.is_ne())
            .unwrap_or(Ordering::Equal)
    }

    fn rank(qualifier: &str) -> String {
        match QUALIFIERS.iter().position(|q| *q == qualifier) {
            Some(index) => index.to_string(),
            None => format!("{}-{qualifier}", QUALIFIERS.len()),
        }
    }

    /// `a1`, `b1` and `m1` are alpha, beta and milestone; `ga`, `final` and
    /// `release` are the release itself, and `cr` is `rc`.
    fn qualifier(text: &str, followed_by_digit: bool) -> Item {
        let text = match (text, followed_by_digit) {
            ("a", true) => "alpha",
            ("b", true) => "beta",
            ("m", true) => "milestone",
            ("ga" | "final" | "release", _) => "",
            ("cr", _) => "rc",
            (text, _) => text,
        };
        Item::Qualifier(text.to_string())
    }

    fn item(text: &str, digits: bool) -> Item {
        if digits {
            let trimmed = text.trim_start_matches('0');
            Item::Number(if trimmed.is_empty() { "0" } else { trimmed }.to_string())
        } else {
            qualifier(text, false)
        }
    }

    /// Drops trailing null items (`0`, the release qualifier, empty lists).
    fn normalize(items: &mut Vec<Item>) {
        for i in (0..items.len()).rev() {
            if items[i].is_null() {
                items.remove(i);
            } else if !matches!(items[i], Item::List(_)) {
                break;
            }
        }
    }

    fn parse(version: &str) -> Vec<Item> {
        let version = version.trim().to_lowercase();
        let mut stack: Vec<Vec<Item>> = vec![Vec::new()];
        let mut digits = false;
        let mut start = 0;
        for (i, c) in version.char_indices() {
            let list = stack.last_mut().expect("the root list stays");
            match c {
                '.' | '-' => {
                    list.push(if i == start {
                        Item::Number("0".into())
                    } else {
                        item(&version[start..i], digits)
                    });
                    start = i + 1;
                    if c == '-' {
                        stack.push(Vec::new());
                    }
                }
                c if c.is_ascii_digit() => {
                    if !digits && i > start {
                        list.push(qualifier(&version[start..i], true));
                        start = i;
                        stack.push(Vec::new());
                    }
                    digits = true;
                }
                _ => {
                    if digits && i > start {
                        list.push(item(&version[start..i], true));
                        start = i;
                        stack.push(Vec::new());
                    }
                    digits = false;
                }
            }
        }
        if version.len() > start {
            let last = item(&version[start..], digits);
            stack.last_mut().expect("the root list stays").push(last);
        }
        while stack.len() > 1 {
            let mut list = stack.pop().expect("more than one list");
            normalize(&mut list);
            stack
                .last_mut()
                .expect("a parent list")
                .push(Item::List(list));
        }
        let mut root = stack.pop().unwrap_or_default();
        normalize(&mut root);
        root
    }

    pub(super) fn compare(a: &str, b: &str) -> Ordering {
        lists(&parse(a), &parse(b))
    }

    /// An alpha, beta, milestone, rc, cr or snapshot qualifier anywhere.
    pub(super) fn is_prerelease(version: &str) -> bool {
        fn any(items: &[Item]) -> bool {
            items.iter().any(|item| match item {
                Item::Qualifier(text) => QUALIFIERS
                    .iter()
                    .position(|q| q == text)
                    .is_some_and(|i| i < RELEASE),
                Item::List(items) => any(items),
                Item::Number(_) => false,
            })
        }
        any(&parse(version))
    }

    /// Maven ranges (`[1.0,2.0)`, `(,1.0]`, `[1.5]`, unions as
    /// `(,1.0],[1.2,)`), Gradle prefixes (`1.+`, `+`), and a bare version,
    /// which Gradle reads as that version or higher.
    pub(super) fn satisfies(range: &str, version: &str) -> Option<bool> {
        let range = range.trim();
        if !version.trim().starts_with(|c: char| c.is_ascii_digit()) {
            return None;
        }
        if let Some(prefix) = range.strip_suffix('+') {
            let readable = prefix.is_empty()
                || prefix.starts_with(|c: char| c.is_ascii_digit()) && prefix.ends_with('.');
            return readable.then(|| version.trim().starts_with(prefix));
        }
        if !range.starts_with(['[', '(']) {
            let bare = range.starts_with(|c: char| c.is_ascii_digit())
                && !range.contains(|c: char| "[](),!+* ".contains(c));
            return bare.then(|| compare(version, range).is_ge());
        }
        let mut rest = range;
        let mut any = false;
        loop {
            let inclusive_low = match rest.chars().next()? {
                '[' => true,
                '(' => false,
                _ => return None,
            };
            let close = rest.find([']', ')'])?;
            let inclusive_high = rest[close..].starts_with(']');
            any |= interval(&rest[1..close], inclusive_low, inclusive_high, version)?;
            rest = rest[close + 1..].trim_start();
            if rest.is_empty() {
                return Some(any);
            }
            rest = rest.strip_prefix(',')?.trim_start();
        }
    }

    fn interval(
        bounds: &str,
        inclusive_low: bool,
        inclusive_high: bool,
        version: &str,
    ) -> Option<bool> {
        let Some((low, high)) = bounds.split_once(',') else {
            // `[1.5]` is exactly 1.5.
            let exact = bounds.trim();
            return (inclusive_low && inclusive_high && !exact.is_empty())
                .then(|| compare(version, exact).is_eq());
        };
        let (low, high) = (low.trim(), high.trim());
        if high.contains(',') {
            return None;
        }
        let above = low.is_empty()
            || match compare(version, low) {
                Ordering::Greater => true,
                Ordering::Equal => inclusive_low,
                Ordering::Less => false,
            };
        let below = high.is_empty()
            || match compare(version, high) {
                Ordering::Less => true,
                Ordering::Equal => inclusive_high,
                Ordering::Greater => false,
            };
        Some(above && below)
    }
}
