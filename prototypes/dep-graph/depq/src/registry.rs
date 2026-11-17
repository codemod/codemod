//! Release metadata from package registries. The engine only asks the
//! `Registry` trait; the host decides how (and whether) to reach the network.

use crate::model::{Eco, Kind};
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    pub yanked: bool,
    /// (dependency name, requirement, kind)
    pub deps: Vec<(String, String, Kind)>,
}

pub trait Registry {
    fn releases(&self, eco: Eco, name: &str) -> Vec<Release>;
}

/// No network: every lookup answers "no releases known".
pub struct Offline;
impl Registry for Offline {
    fn releases(&self, _: Eco, _: &str) -> Vec<Release> {
        vec![]
    }
}

/// Live npm and crates.io (sparse index) lookups, cached on disk. Uses curl so
/// the prototype inherits the machine's proxy and CA settings.
pub struct Http {
    pub cache: PathBuf,
}

impl Http {
    fn get(&self, url: &str, accept: &str) -> Option<String> {
        let key = url.replace(['/', ':', '%', '@'], "_");
        let path = self.cache.join(key);
        if let Ok(t) = std::fs::read_to_string(&path) {
            return Some(t);
        }
        let out = Command::new("curl").args(["-sfL", "--max-time", "30", "-H", &format!("accept: {accept}"), url]).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8(out.stdout).ok()?;
        std::fs::create_dir_all(&self.cache).ok();
        std::fs::write(&path, &text).ok();
        Some(text)
    }
}

impl Registry for Http {
    fn releases(&self, eco: Eco, name: &str) -> Vec<Release> {
        match eco {
            Eco::Npm => {
                let url = format!("https://registry.npmjs.org/{}", name.replace('/', "%2f"));
                self.get(&url, "application/vnd.npm.install-v1+json").map(|t| npm_releases(&t)).unwrap_or_default()
            }
            Eco::Cargo => {
                let n = name.to_lowercase();
                let prefix = match n.len() {
                    1 => "1".to_string(),
                    2 => "2".to_string(),
                    3 => format!("3/{}", &n[..1]),
                    _ => format!("{}/{}", &n[..2], &n[2..4]),
                };
                self.get(&format!("https://index.crates.io/{prefix}/{n}"), "text/plain").map(|t| crate_releases(&t)).unwrap_or_default()
            }
        }
    }
}

pub fn npm_releases(packument: &str) -> Vec<Release> {
    let doc: serde_json::Value = serde_json::from_str(packument).unwrap_or_default();
    let mut out = vec![];
    for (version, meta) in doc.get("versions").and_then(|v| v.as_object()).into_iter().flatten() {
        let mut deps = vec![];
        for (field, kind) in [("dependencies", Kind::Prod), ("peerDependencies", Kind::Peer), ("optionalDependencies", Kind::Optional)] {
            for (n, r) in meta.get(field).and_then(|v| v.as_object()).into_iter().flatten() {
                deps.push((n.clone(), r.as_str().unwrap_or("").to_string(), kind));
            }
        }
        out.push(Release { version: version.clone(), yanked: false, deps });
    }
    out
}

pub fn crate_releases(index: &str) -> Vec<Release> {
    index
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .map(|v| Release {
            version: v["vers"].as_str().unwrap_or("").to_string(),
            yanked: v["yanked"].as_bool().unwrap_or(false),
            deps: v["deps"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|d| {
                    let name = d.get("package").and_then(|p| p.as_str()).or(d["name"].as_str()).unwrap_or("").to_string();
                    let kind = match d["kind"].as_str() {
                        Some("dev") => Kind::Dev,
                        Some("build") => Kind::Build,
                        _ if d["optional"].as_bool() == Some(true) => Kind::Optional,
                        _ => Kind::Prod,
                    };
                    (name, d["req"].as_str().unwrap_or("*").to_string(), kind)
                })
                .collect(),
        })
        .collect()
}
