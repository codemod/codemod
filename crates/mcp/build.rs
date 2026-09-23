use std::env;
use std::fs;
use std::path::PathBuf;

const DOCS: &[&str] = &[
    "README.md",
    "community/cli.mdx",
    "community/model-context-protocol.mdx",
    "community/oss.mdx",
    "community/oss-quickstart.mdx",
    "community/package-structure.mdx",
    "community/workflows.mdx",
    "community/workflows/reference.mdx",
    "community/workflows/sharding.mdx",
    "community/jssg.mdx",
    "community/jssg/reference.mdx",
    "community/jssg/security.mdx",
    "community/jssg/advanced.mdx",
    "community/jssg/testing.mdx",
    "community/jssg/metrics.mdx",
    "community/jssg/utils.mdx",
    "community/jssg/semantic-analysis.mdx",
];

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo_docs_dir = manifest_dir.join("../../docs");
    let out_docs_dir = PathBuf::from(env::var("OUT_DIR").unwrap()).join("docs");

    println!("cargo:rerun-if-changed={}", repo_docs_dir.display());

    for relative_path in DOCS {
        let source = repo_docs_dir.join(relative_path);
        let destination = out_docs_dir.join(relative_path);
        copy_doc(relative_path, source, destination);
    }
}

fn copy_doc(relative_path: &str, source: PathBuf, destination: PathBuf) {
    println!("cargo:rerun-if-changed={}", source.display());

    let content = fs::read_to_string(&source).unwrap_or_else(|error| {
        panic!(
            "failed to read docs/{relative_path} from {}: {error}",
            source.display()
        )
    });

    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|error| {
            panic!(
                "failed to create bundled docs directory {}: {error}",
                parent.display()
            )
        });
    }

    fs::write(&destination, content).unwrap_or_else(|error| {
        panic!(
            "failed to write bundled docs file {}: {error}",
            destination.display()
        )
    });
}
