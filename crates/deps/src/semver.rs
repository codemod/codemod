//! Version ranges per ecosystem: npm ranges follow node-semver, Cargo
//! requirements follow Cargo's rules (`"1.2"` means `^1.2`), and Python
//! versions follow PEP 440, with Poetry's `^`, `~` and `||` read too, and
//! Ruby gems follow RubyGems' `Gem::Version` and `Gem::Requirement`.

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
        Ecosystem::Rubygems => rubygems::satisfies(range, version),
    }
}

pub fn is_prerelease(ecosystem: Ecosystem, version: &str) -> bool {
    match ecosystem {
        Ecosystem::Npm => node_semver::Version::parse(version).map_or(true, |v| v.is_prerelease()),
        Ecosystem::Cargo => semver::Version::parse(version).map_or(true, |v| !v.pre.is_empty()),
        Ecosystem::Pypi => pypi::version(version).is_none_or(|v| v.any_prerelease()),
        Ecosystem::Rubygems => rubygems::Version::parse(version).is_none_or(|v| v.prerelease()),
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
        Ecosystem::Rubygems => by(a, b, rubygems::Version::parse),
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

mod rubygems {
    use std::cmp::Ordering;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Segment {
        Number(u64),
        Letters(String),
    }

    /// A `Gem::Version`: runs of digits and of letters, where any letters
    /// make a prerelease that sorts before its release (`1.0.a < 1.0`).
    pub(super) struct Version {
        segments: Vec<Segment>,
    }

    impl Version {
        pub(super) fn parse(text: &str) -> Option<Self> {
            let text = text.trim();
            let text = if text.is_empty() { "0" } else { text };
            if !text.starts_with(|c: char| c.is_ascii_digit())
                || !text
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
            {
                return None;
            }
            let mut segments = Vec::new();
            let mut run = String::new();
            // RubyGems reads `1.0-rc1` as `1.0.pre.rc1`.
            for c in text.replace('-', ".pre.").chars().chain(['.']) {
                let same = run
                    .chars()
                    .last()
                    .is_some_and(|last| last.is_ascii_digit() == c.is_ascii_digit());
                if !run.is_empty() && (c == '.' || !same) {
                    segments.push(match run.parse() {
                        Ok(number) => Segment::Number(number),
                        Err(_) if run.starts_with(|c: char| c.is_ascii_digit()) => return None,
                        Err(_) => Segment::Letters(run.clone()),
                    });
                    run.clear();
                }
                if c != '.' {
                    run.push(c);
                }
            }
            Some(Self { segments })
        }

        pub(super) fn prerelease(&self) -> bool {
            self.segments
                .iter()
                .any(|s| matches!(s, Segment::Letters(_)))
        }

        /// The segments with trailing zeros dropped from the release part
        /// and from the prerelease part, so `1.0` equals `1.0.0`.
        fn canonical(&self) -> Vec<Segment> {
            let split = self
                .segments
                .iter()
                .position(|s| matches!(s, Segment::Letters(_)))
                .unwrap_or(self.segments.len());
            let (release, pre) = self.segments.split_at(split);
            let trim = |part: &[Segment]| {
                let end = part
                    .iter()
                    .rposition(|s| *s != Segment::Number(0))
                    .map_or(0, |i| i + 1);
                part[..end].to_vec()
            };
            [trim(release), trim(pre)].concat()
        }

        fn release(&self) -> Self {
            let segments = self
                .segments
                .iter()
                .take_while(|s| matches!(s, Segment::Number(_)))
                .cloned()
                .collect();
            Self { segments }
        }

        /// The upper bound of `~>`: the prerelease and the last part
        /// dropped, then the new last part raised (`2.2.0` to `2.3`).
        fn bump(&self) -> Self {
            let mut segments = self.release().segments;
            if segments.len() > 1 {
                segments.pop();
            }
            if let Some(Segment::Number(last)) = segments.last_mut() {
                *last += 1;
            }
            Self { segments }
        }
    }

    impl Ord for Version {
        fn cmp(&self, other: &Self) -> Ordering {
            let (a, b) = (self.canonical(), other.canonical());
            let zero = Segment::Number(0);
            for i in 0..a.len().max(b.len()) {
                let ordering = match (a.get(i).unwrap_or(&zero), b.get(i).unwrap_or(&zero)) {
                    (Segment::Number(a), Segment::Number(b)) => a.cmp(b),
                    (Segment::Letters(a), Segment::Letters(b)) => a.cmp(b),
                    (Segment::Letters(_), Segment::Number(_)) => Ordering::Less,
                    (Segment::Number(_), Segment::Letters(_)) => Ordering::Greater,
                };
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
            Ordering::Equal
        }
    }

    impl PartialOrd for Version {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(self.cmp(other))
        }
    }

    impl PartialEq for Version {
        fn eq(&self, other: &Self) -> bool {
            self.cmp(other).is_eq()
        }
    }

    impl Eq for Version {}

    /// A `Gem::Requirement`: comma-separated clauses that must all hold,
    /// each an operator (`=` when left out) and a version.
    pub(super) fn satisfies(range: &str, version: &str) -> Option<bool> {
        let version = Version::parse(version)?;
        let mut all = true;
        for clause in range.split(',') {
            let clause = clause.trim();
            let operator = ["~>", ">=", "<=", "!=", "=", ">", "<"]
                .into_iter()
                .find(|op| clause.starts_with(op))
                .unwrap_or("=");
            let bound = Version::parse(clause.strip_prefix(operator).unwrap_or(clause))?;
            let ordering = version.cmp(&bound);
            all &= match operator {
                "~>" => ordering.is_ge() && version.release().cmp(&bound.bump()).is_lt(),
                ">=" => ordering.is_ge(),
                "<=" => ordering.is_le(),
                "!=" => ordering.is_ne(),
                ">" => ordering.is_gt(),
                "<" => ordering.is_lt(),
                _ => ordering.is_eq(),
            };
        }
        Some(all)
    }
}
