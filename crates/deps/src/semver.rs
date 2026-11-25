//! Version ranges per ecosystem: npm ranges follow node-semver, Cargo
//! requirements follow Cargo's rules (`"1.2"` means `^1.2`), and Python
//! versions follow PEP 440, with Poetry's `^`, `~` and `||` read too.

use std::cmp::Ordering;

use crate::model::Ecosystem;

/// Whether `version` satisfies `range`; `None` when either cannot be read
/// (`workspace:`, `git:`, `file:` and other non-registry ranges).
pub fn satisfies(ecosystem: Ecosystem, range: &str, version: &str) -> Option<bool> {
    match ecosystem {
        // Any protocol but `npm:` (`file:`, `link:`, `git+https:` ...)
        // names no registry range, even when it ends in `@^1`; an alias
        // ("npm:other@^1") carries its range after the last '@'.
        Ecosystem::Npm => {
            let range = npm_range(range)?;
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

/// Whether `range` is a range the ecosystem's rules can read: not a git,
/// file, link or url spec, nor a tag such as "latest".
pub fn valid_range(ecosystem: Ecosystem, range: &str) -> bool {
    match ecosystem {
        Ecosystem::Npm => npm_range(range).is_some(),
        Ecosystem::Cargo => semver::VersionReq::parse(range).is_ok(),
        Ecosystem::Pypi => pypi::satisfies(range, "0").is_some(),
    }
}

/// An npm range without its `npm:` alias; none for other protocols and tags.
fn npm_range(range: &str) -> Option<node_semver::Range> {
    if range.contains(':') && !range.starts_with("npm:") {
        return None;
    }
    let range = range.strip_prefix("npm:").unwrap_or(range);
    let range = match range.rfind('@') {
        Some(at) if at > 0 => &range[at + 1..],
        _ => range,
    };
    if range.contains(':') {
        return None;
    }
    // node-semver reads a bare word as no range; npm reads it as a dist-tag.
    if range
        .trim()
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && !range.trim().starts_with(['x', 'X', 'v', 'V'])
    {
        return None;
    }
    node_semver::Range::parse(range).ok()
}

/// The lowest version `range` accepts.
pub fn min_version(ecosystem: Ecosystem, range: &str) -> Option<String> {
    match ecosystem {
        Ecosystem::Npm => npm_range(range)?.min_version().map(|v| v.to_string()),
        Ecosystem::Cargo => {
            let req = semver::VersionReq::parse(range).ok()?;
            let mut lowest = semver::Version::new(0, 0, 0);
            for comparator in &req.comparators {
                let at = semver::Version {
                    major: comparator.major,
                    minor: comparator.minor.unwrap_or(0),
                    patch: comparator.patch.unwrap_or(0),
                    pre: comparator.pre.clone(),
                    build: semver::BuildMetadata::EMPTY,
                };
                let bound = match comparator.op {
                    semver::Op::Greater => semver::Version::new(at.major, at.minor, at.patch + 1),
                    semver::Op::Less | semver::Op::LessEq => continue,
                    _ => at,
                };
                if bound > lowest {
                    lowest = bound;
                }
            }
            Some(lowest.to_string())
        }
        Ecosystem::Pypi => {
            let candidates: Vec<String> = range
                .split([',', '|'])
                .filter_map(|clause| {
                    let clause = clause.trim();
                    let version = clause
                        .strip_prefix(">=")
                        .or_else(|| clause.strip_prefix("=="))
                        .or_else(|| clause.strip_prefix("~="))
                        .or_else(|| clause.strip_prefix('^'))
                        .or_else(|| clause.strip_prefix('~'))?;
                    pypi::version(version.trim()).map(|_| version.trim().to_string())
                })
                .collect();
            candidates
                .into_iter()
                .min_by(|a, b| compare(Ecosystem::Pypi, a, b))
        }
    }
}

/// The major version of a version, or of a range's lowest version.
pub fn major(ecosystem: Ecosystem, text: &str) -> Option<u64> {
    let version = match ecosystem {
        Ecosystem::Npm if node_semver::Version::parse(text).is_ok() => text.to_string(),
        Ecosystem::Cargo if semver::Version::parse(text).is_ok() => text.to_string(),
        Ecosystem::Pypi if pypi::version(text).is_some() => text.to_string(),
        _ => min_version(ecosystem, text)?,
    };
    match ecosystem {
        Ecosystem::Npm => node_semver::Version::parse(&version).ok().map(|v| v.major),
        Ecosystem::Cargo => semver::Version::parse(&version).ok().map(|v| v.major),
        Ecosystem::Pypi => pypi::version(&version).and_then(|v| v.release().first().copied()),
    }
}

/// The first version-looking run in `text`, padded to three parts:
/// "v1.2" is "1.2.0", "pnpm@10.1.3+sha" is "10.1.3".
pub fn coerce(text: &str) -> Option<String> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let mut parts: Vec<String> = Vec::new();
    for part in text[start..].split('.') {
        let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            break;
        }
        parts.push(digits.trim_start_matches('0').to_string());
        if parts.len() == 3 || digits.len() != part.len() {
            break;
        }
    }
    let parts: Vec<String> = parts
        .into_iter()
        .map(|p| if p.is_empty() { "0".into() } else { p })
        .collect();
    let mut parts = parts;
    parts.resize(3, "0".into());
    Some(parts.join("."))
}

/// The highest of `versions` that `range` accepts.
pub fn max_satisfying(ecosystem: Ecosystem, versions: &[String], range: &str) -> Option<String> {
    versions
        .iter()
        .filter(|version| satisfies(ecosystem, range, version) == Some(true))
        .max_by(|a, b| compare(ecosystem, a, b))
        .cloned()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_helpers_people_hand_roll() {
        let npm = Ecosystem::Npm;
        assert_eq!(min_version(npm, "^18.2.0").as_deref(), Some("18.2.0"));
        assert_eq!(min_version(npm, ">=1 <2").as_deref(), Some("1.0.0"));
        assert_eq!(
            min_version(npm, "npm:other@~4.1.0").as_deref(),
            Some("4.1.0")
        );
        assert_eq!(min_version(npm, "workspace:*"), None);
        assert_eq!(
            min_version(Ecosystem::Cargo, "0.29").as_deref(),
            Some("0.29.0")
        );
        assert_eq!(
            min_version(Ecosystem::Pypi, ">=2.1,<3").as_deref(),
            Some("2.1")
        );
        assert_eq!(major(npm, "^18.2.0"), Some(18));
        assert_eq!(major(npm, "19.1.0"), Some(19));
        assert_eq!(major(npm, "latest"), None);
        assert_eq!(coerce("v1.2").as_deref(), Some("1.2.0"));
        assert_eq!(coerce("pnpm@10.1.3+sha512.abc").as_deref(), Some("10.1.3"));
        assert_eq!(coerce("none"), None);
        let versions = ["1.0.0", "1.5.0", "2.0.0"].map(String::from);
        assert_eq!(
            max_satisfying(npm, &versions, "^1.0.0").as_deref(),
            Some("1.5.0")
        );
        assert!(valid_range(npm, "^1 || ^2"));
        assert!(valid_range(npm, "*"));
        assert!(!valid_range(npm, "latest"));
        assert!(!valid_range(npm, "github:a/b"));
        assert!(!valid_range(npm, "catalog:"));
    }
}
