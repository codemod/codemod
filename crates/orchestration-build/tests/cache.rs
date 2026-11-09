//! The bundle cache: every change to the workflow graph, a helper bundled
//! only into an artifact, or the cached bundle itself rebuilds.
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use orchestration_build::{cached_quickjs_bundle, private_directory, CachedBundle, Runtime};
use tempfile::TempDir;

fn runtime() -> Runtime {
    let package = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/orchestration");
    Runtime {
        src: fs::canonicalize(package.join("src")).unwrap(),
    }
}

struct Case {
    dir: TempDir,
}

impl Case {
    /// A workflow whose only transform uses a helper from another module.
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let case = Self { dir };
        case.helper("one");
        let index = serde_json::to_string(&runtime().src.join("index.ts")).unwrap();
        fs::write(
            case.workflow(),
            format!(
                r#"import {{ jssg }} from {index};
import {{ rewrite }} from "./helper.ts";
export default jssg({{
  name: "rewrite",
  language: "typescript",
  transform(root) {{
    return rewrite(root.root().text());
  }},
}});
"#
            ),
        )
        .unwrap();
        case
    }

    fn workflow(&self) -> PathBuf {
        self.dir.path().join("workflow.ts")
    }

    fn helper(&self, word: &str) {
        fs::write(
            self.dir.path().join("helper.ts"),
            format!("export const rewrite = (text: string): string => text.replace(\"a\", \"{word}\");\n"),
        )
        .unwrap();
    }

    async fn bundle(&self) -> CachedBundle {
        cached_quickjs_bundle(&self.workflow(), &runtime(), &self.dir.path().join("cache"))
            .await
            .unwrap()
    }
}

#[test]
fn a_cache_directory_others_can_write_to_is_not_used() {
    let root = TempDir::new().unwrap();
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(private_directory("quickjs", root.path()), None);
}

#[test]
fn a_private_cache_directory_is_created_inside_the_root() {
    let root = TempDir::new().unwrap();
    assert_eq!(
        private_directory("quickjs", root.path()),
        Some(root.path().join("quickjs"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_first_build_is_not_cached() {
    assert!(!Case::new().bundle().await.cached);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unchanged_workflow_is_served_from_the_cache() {
    let case = Case::new();
    let first = case.bundle().await;
    assert_eq!(
        case.bundle().await,
        CachedBundle {
            cached: true,
            ..first
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_changed_helper_bundled_only_into_an_artifact_rebuilds() {
    let case = Case::new();
    case.bundle().await;
    case.helper("two");
    let rebuilt = case.bundle().await;
    assert!(fs::read_to_string(&rebuilt.path).unwrap().contains("two"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cached_bundle_that_no_longer_matches_its_hash_is_rebuilt() {
    let case = Case::new();
    let first = case.bundle().await;
    fs::write(&first.path, "export function main() {}\n").unwrap();
    assert_eq!(
        case.bundle().await,
        CachedBundle {
            cached: false,
            ..first
        }
    );
}
