//! Codemod 1.x compatibility in the translation `workflow run` uses: which
//! of the 1.x transform APIs 2.0 removed a package uses, and the helpers the
//! translated workflow then needs. The APIs themselves are `js/v1.ts`, which
//! the isolates of a workflow.yaml run provide (`tasks/isolate.rs`).

use std::path::Path;

/// What a package's code uses of the removed 1.x APIs.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Usage {
    /// `getState`, `setState`, `unsetState` or `acquireLock` from
    /// `codemod:workflow`.
    pub state: bool,
    /// `codemod:metrics`.
    pub metrics: bool,
}

/// Files bigger than this are not read (bundled dependencies, data).
const MAX_FILE: u64 = 8 * 1024 * 1024;
/// At most this many files are read.
const MAX_FILES: usize = 5_000;

const STATE_API: &[&str] = &["getState", "setState", "unsetState", "acquireLock"];

/// Which removed 1.x APIs the JavaScript and TypeScript files of the package
/// in `dir` use, outside `node_modules` and hidden directories. A plain text
/// search: a mention is enough, as using one where it is not provided fails
/// anyway.
pub(crate) fn usage(dir: &Path) -> Usage {
    let mut usage = Usage::default();
    let mut pending = vec![dir.to_path_buf()];
    let mut read = 0;
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if name != "node_modules" && !name.starts_with('.') {
                    pending.push(path);
                }
                continue;
            }
            let script = ["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"]
                .iter()
                .any(|ext| name.ends_with(&format!(".{ext}")));
            if !script || name.ends_with(".d.ts") {
                continue;
            }
            if entry.metadata().map(|m| m.len() > MAX_FILE).unwrap_or(true) {
                continue;
            }
            read += 1;
            if read > MAX_FILES {
                return usage;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            usage.metrics |= text.contains("codemod:metrics") || text.contains("codemod/metrics");
            usage.state |= (text.contains("codemod:workflow") || text.contains("codemod/workflow"))
                && STATE_API.iter().any(|api| text.contains(api));
            if usage.state && usage.metrics {
                return usage;
            }
        }
    }
    usage
}

/// Appended to a translated workflow whose transforms use the 1.x state or
/// metrics. A jssg step's result carries what its transforms set and counted
/// (`v1`, from `tasks/v1_host.rs`): `v1Apply` copies the state into the
/// workflow's and adds up the counts, which the run returns (`v1Report`).
/// The body only reads recorded step results, so it replays as it ran.
pub(crate) const HELPERS: &str = r#"
/** Codemod 1.x compatibility: the counts the transforms made with `codemod:metrics`, by metric and cardinality. */
const v1Metrics: Record<string, Record<string, { cardinality: Record<string, string>; count: number }>> = {};

/** Codemod 1.x compatibility: copy what a jssg step's transforms set into `state`, and add up what they counted. */
function v1Apply<T>(result: T, state?: Record<string, any>): T {
  const v1 = (result as { v1?: any } | null | undefined)?.v1;
  if (v1?.state && state) {
    Object.assign(state, v1.state.set ?? {});
    for (const key of v1.state.unset ?? []) delete state[key];
  }
  for (const [name, entries] of Object.entries(v1?.metrics ?? {}) as [string, any[]][]) {
    const metric = (v1Metrics[name] ??= {});
    for (const { cardinality, count } of entries) {
      const key = JSON.stringify(cardinality);
      metric[key] = { cardinality, count: (metric[key]?.count ?? 0) + count };
    }
  }
  return result;
}

/** Codemod 1.x compatibility: the run's output, the metrics its transforms counted. */
function v1Report() {
  const names = Object.keys(v1Metrics).sort();
  if (names.length === 0) return undefined;
  return { metrics: Object.fromEntries(names.map((name) => [name, Object.values(v1Metrics[name])])) };
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_apis_a_package_uses() {
        let dir = tempfile::tempdir().unwrap();
        let scripts = dir.path().join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/x")).unwrap();
        assert_eq!(usage(dir.path()), Usage::default());

        std::fs::write(
            dir.path().join("node_modules/x/index.js"),
            "import { useMetricAtom } from \"codemod:metrics\";",
        )
        .unwrap();
        std::fs::write(
            scripts.join("types.d.ts"),
            "declare module \"codemod:metrics\" {}",
        )
        .unwrap();
        assert_eq!(usage(dir.path()), Usage::default());

        std::fs::write(
            scripts.join("codemod.ts"),
            "import { useMetricAtom } from \"codemod:metrics\";",
        )
        .unwrap();
        assert_eq!(
            usage(dir.path()),
            Usage {
                state: false,
                metrics: true
            }
        );
        std::fs::write(
            scripts.join("state.ts"),
            "import { getState } from 'codemod:workflow';",
        )
        .unwrap();
        assert_eq!(
            usage(dir.path()),
            Usage {
                state: true,
                metrics: true
            }
        );
    }
}
