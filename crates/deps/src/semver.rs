//! Version ranges per ecosystem: npm ranges follow node-semver, Cargo
//! requirements follow Cargo's rules (`"1.2"` means `^1.2`), and Python
//! versions follow PEP 440, with Poetry's `^`, `~` and `||` read too.

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
    }
}

pub fn is_prerelease(ecosystem: Ecosystem, version: &str) -> bool {
    match ecosystem {
        Ecosystem::Npm => node_semver::Version::parse(version).map_or(true, |v| v.is_prerelease()),
        Ecosystem::Cargo => semver::Version::parse(version).map_or(true, |v| !v.pre.is_empty()),
        Ecosystem::Pypi => pypi::version(version).is_none_or(|v| v.any_prerelease()),
    }
}

pub fn compare(ecosystem: Ecosystem, a: &str, b: &str) -> Ordering {
    let ordered = match ecosystem {
        Ecosystem::Npm => node_semver::Version::parse(a)
            .ok()
            .zip(node_semver::Version::parse(b).ok())
            .map(|(a, b)| a.cmp(&b)),
        Ecosystem::Cargo => semver::Version::parse(a)
            .ok()
            .zip(semver::Version::parse(b).ok())
            .map(|(a, b)| a.cmp(&b)),
        Ecosystem::Pypi => pypi::version(a)
            .zip(pypi::version(b))
            .map(|(a, b)| a.cmp(&b)),
    };
    ordered.unwrap_or_else(|| a.cmp(b))
}

/// Whether going from `from` to `to` is a breaking change under caret rules:
/// the left-most non-zero component changes (so 0.29 to 0.30 is breaking).
/// Python versions compare their epoch and release numbers, so `0.1rc1` to
/// `0.1` is not breaking.
pub fn is_breaking(ecosystem: Ecosystem, from: &str, to: &str) -> bool {
    if ecosystem == Ecosystem::Pypi {
        if let Some((from, to)) = pypi::version(from).zip(pypi::version(to)) {
            return pypi::breaking_part(&from) != pypi::breaking_part(&to);
        }
    }
    fn breaking_part(version: &str) -> Vec<&str> {
        let parts: Vec<&str> = version.split(['.', '-', '+']).take(3).collect();
        match parts.iter().position(|part| *part != "0") {
            Some(index) => parts[..=index].to_vec(),
            None => parts,
        }
    }
    breaking_part(from) != breaking_part(to)
}

mod pypi {
    use std::str::FromStr;

    use pep440_rs::{Version, VersionSpecifier};

    pub(super) fn version(text: &str) -> Option<Version> {
        Version::from_str(text).ok()
    }

    /// The epoch and the release numbers (padded to three) up to the
    /// left-most non-zero one.
    pub(super) fn breaking_part(version: &Version) -> (u64, Vec<u64>) {
        let mut parts: Vec<u64> = version.release().iter().copied().take(3).collect();
        parts.resize(3, 0);
        if let Some(index) = parts.iter().position(|&part| part != 0) {
            parts.truncate(index + 1);
        }
        (version.epoch(), parts)
    }

    /// A PEP 440 specifier set, or a Poetry constraint: alternatives joined
    /// by `||`, each a comma-separated list of clauses that must all hold.
    pub(super) fn satisfies(range: &str, version: &str) -> Option<bool> {
        let version = self::version(version)?;
        let mut any = false;
        for alternative in range.split("||") {
            let mut all = true;
            for clause in alternative.split(',') {
                let clause: String = clause.split_whitespace().collect();
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

    /// One clause as PEP 440 specifiers: Poetry's `^1.2` is `>=1.2,<2`,
    /// `~1.2` is `>=1.2,<1.3`, and a bare `1.2` is `==1.2`. The operand of
    /// `^` and `~` is any PEP 440 version (`^1.2rc1`, `~1!2.0`); the upper
    /// bound comes from its release numbers and keeps its epoch.
    fn specifiers(clause: &str) -> Option<Vec<String>> {
        if clause.is_empty() || clause == "*" {
            return Some(Vec::new());
        }
        if let Some(lower) = clause.strip_prefix('^') {
            let lower = version(lower)?;
            let parts = lower.release();
            let at = parts.iter().position(|&p| p != 0);
            return between(&lower, at.unwrap_or(parts.len() - 1));
        }
        if let Some(lower) = clause
            .strip_prefix('~')
            .filter(|rest| !rest.starts_with('='))
        {
            let lower = version(lower)?;
            let at = usize::from(lower.release().len() > 1);
            return between(&lower, at);
        }
        if clause.starts_with(|c: char| c.is_ascii_digit()) {
            return Some(vec![format!("=={clause}")]);
        }
        Some(vec![clause.to_string()])
    }

    /// `>=lower` and below `lower`'s release with the part at `at` raised
    /// by one; `None` when that part cannot be raised, so the clause cannot
    /// be read.
    fn between(lower: &Version, at: usize) -> Option<Vec<String>> {
        let parts = lower.release();
        let mut upper = parts[..at].to_vec();
        upper.push(parts[at].checked_add(1)?);
        let upper = Version::new(upper).with_epoch(lower.epoch());
        Some(vec![format!(">={lower}"), format!("<{upper}")])
    }
}
