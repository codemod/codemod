//! Embeds the portable orchestration runtime's sources
//! (`packages/orchestration/src`) and the npm packages it bundles, so a built
//! binary needs no checkout.
use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// npm packages the runtime imports in QuickJS, embedded as `node_modules/<name>`.
const PACKAGES: &[&str] = &["ignore"];

fn collect(dir: &Path, root: &Path, prefix: &str, files: &mut Vec<(String, PathBuf)>) {
    for entry in fs::read_dir(dir).expect("readable runtime sources") {
        let path = entry.expect("readable runtime sources").path();
        if path.is_dir() {
            collect(&path, root, prefix, files);
            continue;
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let source =
            name.ends_with(".ts") && !name.ends_with(".test.ts") && !name.ends_with(".d.ts");
        if source || name.ends_with(".json") || name.ends_with(".js") {
            let relative = path.strip_prefix(root).unwrap().to_str().unwrap();
            files.push((
                format!("{prefix}{}", relative.replace('\\', "/")),
                path.canonicalize().unwrap(),
            ));
        }
    }
}

fn main() {
    let package = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap())
        .join("../../packages/orchestration")
        .canonicalize()
        .expect("packages/orchestration next to this crate");
    let src = package.join("src");
    println!("cargo:rerun-if-changed={}", src.display());
    let mut files = Vec::new();
    collect(&src, &src, "", &mut files);
    for name in PACKAGES {
        let root = package
            .join("node_modules")
            .join(name)
            .canonicalize()
            .unwrap_or_else(|_| panic!("npm package '{name}' is missing; run pnpm install"));
        println!("cargo:rerun-if-changed={}", root.display());
        collect(&root, &root, &format!("node_modules/{name}/"), &mut files);
    }
    files.sort();
    let entries: String = files
        .iter()
        .map(|(relative, path)| format!("    ({relative:?}, include_bytes!({:?})),\n", path))
        .collect();
    fs::write(
        Path::new(&env::var("OUT_DIR").unwrap()).join("runtime_files.rs"),
        format!("static RUNTIME_FILES: &[(&str, &[u8])] = &[\n{entries}];\n"),
    )
    .unwrap();
}
