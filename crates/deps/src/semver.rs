//! Version ranges per ecosystem: npm ranges follow node-semver, Cargo
//! requirements follow Cargo's rules (`"1.2"` means `^1.2`).

use std::cmp::Ordering;

use crate::model::Ecosystem;

/// Whether `version` satisfies `range`; `None` when either cannot be read
/// (`workspace:`, `git:`, `file:` and other non-registry ranges).
pub fn satisfies(ecosystem: Ecosystem, range: &str, version: &str) -> Option<bool> {
    match ecosystem {
        Ecosystem::Npm => {
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
    }
}

pub fn is_prerelease(ecosystem: Ecosystem, version: &str) -> bool {
    match ecosystem {
        Ecosystem::Npm => node_semver::Version::parse(version).map_or(true, |v| v.is_prerelease()),
        Ecosystem::Cargo => semver::Version::parse(version).map_or(true, |v| !v.pre.is_empty()),
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
    };
    ordered.unwrap_or_else(|| a.cmp(b))
}

/// Whether going from `from` to `to` is a breaking change under caret rules:
/// the left-most non-zero component changes (so 0.29 to 0.30 is breaking).
pub fn is_breaking(from: &str, to: &str) -> bool {
    fn breaking_part(version: &str) -> Vec<&str> {
        let parts: Vec<&str> = version.split(['.', '-', '+']).take(3).collect();
        match parts.iter().position(|part| *part != "0") {
            Some(index) => parts[..=index].to_vec(),
            None => parts,
        }
    }
    breaking_part(from) != breaking_part(to)
}
