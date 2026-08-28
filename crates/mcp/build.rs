use std::env;
use std::fs;
use std::path::PathBuf;

const DOCS: &[&str] = &[
    "README.md",
    "cli.mdx",
    "model-context-protocol.mdx",
    "oss.mdx",
    "oss-quickstart.mdx",
    "package-structure.mdx",
    "workflows/introduction.mdx",
    "workflows/reference.mdx",
    "workflows/sharding.mdx",
    "jssg/intro.mdx",
    "jssg/reference.mdx",
    "jssg/security.mdx",
    "jssg/advanced.mdx",
    "jssg/testing.mdx",
    "jssg/metrics.mdx",
    "jssg/utils.mdx",
    "jssg/semantic-analysis.mdx",
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
