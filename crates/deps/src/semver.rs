//! Version ranges per ecosystem: npm ranges follow node-semver, Cargo
//! requirements follow Cargo's rules (`"1.2"` means `^1.2`), and Python
//! versions follow PEP 440, with Poetry's `^`, `~` and `||` read too.
//! Composer versions and constraints follow Composer's own rules.

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
        Ecosystem::Packagist => packagist::satisfies(range, version),
    }
}

pub fn is_prerelease(ecosystem: Ecosystem, version: &str) -> bool {
    match ecosystem {
        Ecosystem::Npm => node_semver::Version::parse(version).map_or(true, |v| v.is_prerelease()),
        Ecosystem::Cargo => semver::Version::parse(version).map_or(true, |v| !v.pre.is_empty()),
        Ecosystem::Pypi => pypi::version(version).is_none_or(|v| v.any_prerelease()),
        Ecosystem::Packagist => packagist::version(version).is_none_or(|v| v.is_prerelease()),
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
        Ecosystem::Packagist => by(a, b, packagist::version),
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

mod packagist {
    /// A Composer version: four numbers, then a stability ordered dev,
    /// alpha, beta, RC, stable, patch, with its own number (`RC2`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    pub(super) struct Version {
        parts: [u64; 4],
        stability: u8,
        number: u64,
    }

    const DEV: u8 = 0;
    const STABLE: u8 = 4;
    /// What Composer reads `x` as in a branch version such as `2.x-dev`.
    const BRANCH: u64 = 9_999_999;

    impl Version {
        pub(super) fn is_prerelease(&self) -> bool {
            self.stability < STABLE
        }
    }

    /// A version as written: its numbers (`None` for `x` or `*`) and its
    /// stability, if one was given.
    struct Written {
        parts: Vec<Option<u64>>,
        stability: Option<(u8, u64)>,
    }

    impl Written {
        fn parse(text: &str) -> Option<Self> {
            let text = text.strip_prefix(['v', 'V']).unwrap_or(text);
            let end = text
                .find(|c: char| !(c.is_ascii_digit() || matches!(c, '.' | 'x' | 'X' | '*')))
                .unwrap_or(text.len());
            let numbers = text[..end].trim_end_matches('.');
            let parts = numbers
                .split('.')
                .map(|part| match part {
                    "x" | "X" | "*" => Some(None),
                    part => part.parse().ok().map(Some),
                })
                .collect::<Option<Vec<_>>>()?;
            if parts.len() > 4 || parts[0].is_none() && parts.len() > 1 {
                return None;
            }
            let suffix = text[numbers.len()..].trim_start_matches(['.', '-', '_']);
            let stability = if suffix.is_empty() {
                None
            } else {
                Some(stability(suffix)?)
            };
            Some(Self { parts, stability })
        }

        /// The version itself; `x` parts only in a dev branch (`2.x-dev`).
        fn version(&self) -> Option<Version> {
            let (stability, number) = self.stability.unwrap_or((STABLE, 0));
            let mut parts = [0; 4];
            for (at, part) in self.parts.iter().enumerate() {
                parts[at] = match part {
                    Some(n) => *n,
                    None if stability == DEV => BRANCH,
                    None => return None,
                };
            }
            Some(Version {
                parts,
                stability,
                number,
            })
        }

        /// As a lower bound: a version with no stability starts at its
        /// dev release, so `>=1.0` accepts 1.0.0-beta1.
        fn lower(&self) -> Option<Version> {
            let version = self.version()?;
            Some(match self.stability {
                None => dev(version),
                Some(_) => version,
            })
        }

        /// The dev release of this version with the part at `at` (0-based)
        /// raised by one and the parts after it zeroed.
        fn raised(&self, at: usize) -> Option<Version> {
            let mut parts = [0; 4];
            for (index, part) in self.parts.iter().enumerate().take(at + 1) {
                parts[index] = (*part)?;
            }
            parts[at] += 1;
            Some(Version {
                parts,
                stability: DEV,
                number: 0,
            })
        }
    }

    fn dev(version: Version) -> Version {
        Version {
            stability: DEV,
            number: 0,
            ..version
        }
    }

    /// `beta2`, `RC.1`, `patch1`, `dev` as a stability and its number.
    fn stability(suffix: &str) -> Option<(u8, u64)> {
        let lower = suffix.to_ascii_lowercase();
        let (rank, rest) = [
            ("stable", STABLE),
            ("alpha", 1),
            ("a", 1),
            ("beta", 2),
            ("b", 2),
            ("rc", 3),
            ("patch", 5),
            ("pl", 5),
            ("p", 5),
            ("dev", DEV),
        ]
        .iter()
        .find_map(|(word, rank)| Some((*rank, lower.strip_prefix(word)?)))?;
        let rest = rest.trim_start_matches(['.', '-']);
        let number = if rest.is_empty() {
            0
        } else {
            rest.parse().ok()?
        };
        Some((rank, number))
    }

    /// A version Composer can order; `None` for branches like `dev-main`.
    pub(super) fn version(text: &str) -> Option<Version> {
        Written::parse(text.trim())?.version()
    }

    /// A test a version must pass against a bound, such as `Version::ge`.
    type Test = fn(&Version, &Version) -> bool;
    type Bound = (Test, Version);

    /// A Composer constraint: alternatives split by `||` (or `|`), each a
    /// list of clauses split by commas or spaces that must all hold. A
    /// stability flag (`@beta`) is ignored, and of `1.0.x-dev as 1.0.0`
    /// the left side is read. `None` when a part cannot be read.
    pub(super) fn satisfies(range: &str, version: &str) -> Option<bool> {
        let version = self::version(version)?;
        let range = range.split(" as ").next().unwrap_or_default();
        let mut any = false;
        for alternative in range.split('|').filter(|a| !a.trim().is_empty()) {
            any |= bounds(alternative)?
                .iter()
                .all(|(test, bound)| test(&version, bound));
        }
        Some(any || range.trim().is_empty())
    }

    fn bounds(alternative: &str) -> Option<Vec<Bound>> {
        if let Some((from, to)) = alternative.split_once(" - ") {
            return hyphen(from.trim(), to.trim());
        }
        let mut tokens: Vec<String> = Vec::new();
        let mut operator = String::new();
        for token in alternative.split([',', ' ']).filter(|t| !t.is_empty()) {
            if token.chars().all(|c| matches!(c, '<' | '>' | '=' | '!')) {
                operator = token.to_string();
            } else {
                tokens.push(std::mem::take(&mut operator) + token);
            }
        }
        let mut all = Vec::new();
        for token in tokens {
            all.extend(clause(&token)?);
        }
        Some(all)
    }

    /// `1.0 - 2.1` is `>=1.0 <2.2`; a full upper version is kept as `<=`.
    fn hyphen(from: &str, to: &str) -> Option<Vec<Bound>> {
        let (from, to) = (Written::parse(from)?, Written::parse(to)?);
        let upper = if to.parts.len() >= 3 || to.stability.is_some() {
            (Version::le as _, to.version()?)
        } else {
            (Version::lt as _, to.raised(to.parts.len() - 1)?)
        };
        Some(vec![(Version::ge, from.lower()?), upper])
    }

    fn clause(token: &str) -> Option<Vec<Bound>> {
        let token = token.split(['@', '#']).next().unwrap_or_default();
        if token
            .trim_start_matches(['v', 'V'])
            .chars()
            .all(|c| matches!(c, '*' | 'x' | 'X' | '.'))
        {
            return Some(Vec::new());
        }
        if let Some(rest) = token.strip_prefix('^') {
            let written = Written::parse(rest)?;
            let parts: Vec<u64> = written.parts.iter().copied().collect::<Option<_>>()?;
            let at = if parts[0] != 0 || parts.len() == 1 {
                0
            } else if parts[1] != 0 || parts.len() == 2 {
                1
            } else {
                2
            };
            return Some(vec![
                (Version::ge, written.lower()?),
                (Version::lt, written.raised(at)?),
            ]);
        }
        if let Some(rest) = token.strip_prefix('~') {
            let written = Written::parse(rest)?;
            let at = written.parts.len().saturating_sub(2);
            return Some(vec![
                (Version::ge, written.lower()?),
                (Version::lt, written.raised(at)?),
            ]);
        }
        // `<` and `>=` bound a version with no stability at its dev release.
        let operators: [(&str, Test, bool); 8] = [
            (">=", Version::ge, true),
            ("<=", Version::le, false),
            ("<>", Version::ne, false),
            ("!=", Version::ne, false),
            ("==", Version::eq, false),
            (">", Version::gt, false),
            ("<", Version::lt, true),
            ("=", Version::eq, false),
        ];
        let (test, from_dev, rest) = operators
            .iter()
            .find_map(|&(text, test, dev)| Some((test, dev, token.strip_prefix(text)?)))
            .unwrap_or((Version::eq, false, token));
        let written = Written::parse(rest)?;
        // A wildcard: `1.2.*` is `>=1.2.0-dev <1.3.0-dev`. A branch
        // (`2.x-dev`) is one version.
        let wildcard = written.parts.iter().position(Option::is_none);
        if let Some(at) = wildcard.filter(|_| written.stability.is_none()) {
            if rest.len() != token.len() || at == 0 {
                return None;
            }
            let fixed = Written {
                parts: written.parts[..at].to_vec(),
                stability: None,
            };
            return Some(vec![
                (Version::ge, fixed.lower()?),
                (Version::lt, fixed.raised(at - 1)?),
            ]);
        }
        let version = if from_dev {
            written.lower()?
        } else {
            written.version()?
        };
        Some(vec![(test, version)])
    }
}
