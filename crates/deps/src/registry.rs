//! Release metadata from package registries. Queries only see the
//! [`Registry`] trait; the host decides whether and how the network is
//! reached. A workflow step records its result, so a resumed run replays it
//! without asking again.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use futures::{StreamExt, TryStreamExt};

use crate::model::{Ecosystem, Kind};
use crate::semver::{compare, is_prerelease};

#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    pub yanked: bool,
    /// (dependency name, requirement, kind)
    pub dependencies: Vec<(String, String, Kind)>,
}

pub trait Registry: Send + Sync {
    /// Every release of each named package. A package the registry does not
    /// know maps to an empty list; an unreachable registry is an error.
    fn releases(
        &self,
        ecosystem: Ecosystem,
        names: &[String],
    ) -> Result<BTreeMap<String, Vec<Release>>, String>;
}

/// npm (abbreviated packuments) and crates.io (sparse index) over HTTPS,
/// kept for the life of the value. Concurrent asks for the same package
/// share one request, and at most [`CONCURRENT_REQUESTS`] run at once per ask.
#[derive(Default)]
pub struct HttpRegistry {
    slots: Mutex<HashMap<(Ecosystem, String), Slot>>,
}

type Slot = Arc<Mutex<Option<Vec<Release>>>>;

pub const CONCURRENT_REQUESTS: usize = 16;

impl HttpRegistry {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Registry for HttpRegistry {
    fn releases(
        &self,
        ecosystem: Ecosystem,
        names: &[String],
    ) -> Result<BTreeMap<String, Vec<Release>>, String> {
        let unique: BTreeSet<&String> = names.iter().collect();
        let slots: Vec<(&String, Slot)> = {
            let mut all = self.slots.lock().unwrap_or_else(|e| e.into_inner());
            unique
                .into_iter()
                .map(|name| {
                    let slot = all.entry((ecosystem, name.clone())).or_default();
                    (name, Arc::clone(slot))
                })
                .collect()
        };
        // Every caller locks slots in name order, so a second ask for a
        // package waits for the first one's answer instead of refetching.
        let mut held: Vec<_> = slots
            .iter()
            .map(|(name, slot)| (*name, slot.lock().unwrap_or_else(|e| e.into_inner())))
            .collect();
        let missing: Vec<String> = held
            .iter()
            .filter(|(_, slot)| slot.is_none())
            .map(|(name, _)| (*name).clone())
            .collect();
        if !missing.is_empty() {
            // Callers may already be inside an async runtime; fetch on a
            // thread of our own.
            let fetched: BTreeMap<String, Vec<Release>> = std::thread::scope(|scope| {
                scope
                    .spawn(|| {
                        tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .map_err(|e| e.to_string())?
                            .block_on(fetch_all(ecosystem, &missing))
                    })
                    .join()
                    .map_err(|_| "the registry fetch panicked".to_string())?
            })?
            .into_iter()
            .collect();
            for (name, slot) in &mut held {
                if let Some(releases) = fetched.get(*name) {
                    **slot = Some(releases.clone());
                }
            }
        }
        Ok(held
            .into_iter()
            .map(|(name, slot)| (name.clone(), slot.clone().unwrap_or_default()))
            .collect())
    }
}

/// A package's releases as a workflow sees them: oldest first, with
/// dependencies grouped by kind.
pub fn describe(ecosystem: Ecosystem, name: &str, releases: &[Release]) -> serde_json::Value {
    let mut sorted: Vec<&Release> = releases.iter().collect();
    sorted.sort_by(|a, b| compare(ecosystem, &a.version, &b.version));
    let versions: Vec<serde_json::Value> = sorted
        .into_iter()
        .map(|release| {
            let mut groups = serde_json::Map::new();
            for (field, kind) in [
                ("dependencies", Kind::Prod),
                ("peerDependencies", Kind::Peer),
                ("optionalDependencies", Kind::Optional),
                ("buildDependencies", Kind::Build),
                ("devDependencies", Kind::Dev),
            ] {
                let group: serde_json::Map<String, serde_json::Value> = release
                    .dependencies
                    .iter()
                    .filter(|(_, _, k)| *k == kind)
                    .map(|(name, range, _)| (name.clone(), range.clone().into()))
                    .collect();
                if !group.is_empty() || kind != Kind::Build && kind != Kind::Dev {
                    groups.insert(field.into(), group.into());
                }
            }
            let mut entry = serde_json::json!({
                "version": release.version,
                "yanked": release.yanked,
                "prerelease": is_prerelease(ecosystem, &release.version),
            });
            entry.as_object_mut().unwrap().extend(groups);
            entry
        })
        .collect();
    serde_json::json!({ "ecosystem": ecosystem.as_str(), "name": name, "versions": versions })
}

async fn fetch_all(
    ecosystem: Ecosystem,
    names: &[String],
) -> Result<Vec<(String, Vec<Release>)>, String> {
    let client = reqwest::Client::builder()
        .user_agent("codemod-workflow")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let fetches = names.iter().map(|name| {
        let client = &client;
        async move {
            let (url, accept) = match ecosystem {
                Ecosystem::Npm => (
                    format!("https://registry.npmjs.org/{}", name.replace('/', "%2f")),
                    "application/vnd.npm.install-v1+json",
                ),
                Ecosystem::Cargo => (
                    format!("https://index.crates.io/{}", sparse_index_path(name)),
                    "text/plain",
                ),
            };
            let response = client
                .get(&url)
                .header("accept", accept)
                .send()
                .await
                .map_err(|e| format!("{url}: {e}"))?;
            if response.status() == reqwest::StatusCode::NOT_FOUND {
                return Ok((name.clone(), Vec::new()));
            }
            let text = response
                .error_for_status()
                .map_err(|e| format!("{url}: {e}"))?
                .text()
                .await
                .map_err(|e| format!("{url}: {e}"))?;
            let releases = match ecosystem {
                Ecosystem::Npm => npm_releases(&text),
                Ecosystem::Cargo => crate_releases(&text),
            };
            Ok((name.clone(), releases))
        }
    });
    futures::stream::iter(fetches)
        .buffer_unordered(CONCURRENT_REQUESTS)
        .try_collect()
        .await
}

fn sparse_index_path(name: &str) -> String {
    let name = name.to_lowercase();
    match name.len() {
        1 => format!("1/{name}"),
        2 => format!("2/{name}"),
        3 => format!("3/{}/{name}", &name[..1]),
        _ => format!("{}/{}/{name}", &name[..2], &name[2..4]),
    }
}

/// Releases from an npm packument (full or abbreviated).
pub fn npm_releases(packument: &str) -> Vec<Release> {
    let doc: serde_json::Value = serde_json::from_str(packument).unwrap_or_default();
    let versions = doc.get("versions").and_then(|v| v.as_object());
    versions
        .into_iter()
        .flatten()
        .map(|(version, meta)| Release {
            version: version.clone(),
            yanked: false,
            dependencies: crate::lockfiles::npm_sections(meta)
                .into_iter()
                .filter(|(_, _, kind)| *kind != Kind::Dev)
                .collect(),
        })
        .collect()
}

/// Releases from a crates.io sparse index file (one JSON object per line).
pub fn crate_releases(index: &str) -> Vec<Release> {
    index
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .map(|release| Release {
            version: release["vers"].as_str().unwrap_or_default().to_string(),
            yanked: release["yanked"].as_bool().unwrap_or(false),
            dependencies: release["deps"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|dependency| {
                    let name = dependency["package"]
                        .as_str()
                        .or(dependency["name"].as_str())
                        .unwrap_or_default();
                    let kind = match dependency["kind"].as_str() {
                        Some("dev") => Kind::Dev,
                        Some("build") => Kind::Build,
                        _ if dependency["optional"].as_bool() == Some(true) => Kind::Optional,
                        _ => Kind::Prod,
                    };
                    (
                        name.to_string(),
                        dependency["req"].as_str().unwrap_or("*").to_string(),
                        kind,
                    )
                })
                .collect(),
        })
        .collect()
}
