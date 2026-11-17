//! Range checks per ecosystem: npm ranges follow node-semver, Cargo
//! requirements follow Cargo's semver rules ("1.2" means "^1.2").

use crate::model::Eco;

/// `None` when the range or version can't be read (workspace:, git, file: ...).
pub fn satisfies(eco: Eco, req: &str, version: &str) -> Option<bool> {
    match eco {
        Eco::Npm => {
            let req = req.strip_prefix("npm:").unwrap_or(req);
            // An alias "npm:other@^1" carries its range after the last '@'.
            let req = match req.rfind('@') {
                Some(i) if i > 0 => &req[i + 1..],
                _ => req,
            };
            if req.contains(':') {
                return None;
            }
            let range = node_semver::Range::parse(req).ok()?;
            let v = node_semver::Version::parse(version).ok()?;
            Some(range.satisfies(&v))
        }
        Eco::Cargo => {
            let r = semver::VersionReq::parse(req).ok()?;
            let v = semver::Version::parse(version).ok()?;
            Some(r.matches(&v))
        }
    }
}

pub fn is_prerelease(eco: Eco, version: &str) -> bool {
    match eco {
        Eco::Npm => node_semver::Version::parse(version).map(|v| v.is_prerelease()).unwrap_or(true),
        Eco::Cargo => semver::Version::parse(version).map(|v| !v.pre.is_empty()).unwrap_or(true),
    }
}

pub fn cmp(eco: Eco, a: &str, b: &str) -> std::cmp::Ordering {
    match eco {
        Eco::Npm => match (node_semver::Version::parse(a), node_semver::Version::parse(b)) {
            (Ok(a), Ok(b)) => a.cmp(&b),
            _ => a.cmp(b),
        },
        Eco::Cargo => match (semver::Version::parse(a), semver::Version::parse(b)) {
            (Ok(a), Ok(b)) => a.cmp(&b),
            _ => a.cmp(b),
        },
    }
}

/// The left-most non-zero component, which is what a breaking change bumps in both ecosystems.
pub fn breaking_part(version: &str) -> String {
    let parts: Vec<&str> = version.split(['.', '-', '+']).take(3).collect();
    match parts.iter().position(|p| *p != "0") {
        Some(i) => parts[..=i].join("."),
        None => version.to_string(),
    }
}
