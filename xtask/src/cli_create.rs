use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::Command;

pub async fn create_codemod(target_dir: Option<PathBuf>) -> Result<()> {
    // Find the codemod binary
    let xtask_path = std::env::var("CARGO_MANIFEST_DIR")
        .map_err(|_| anyhow::anyhow!("Cannot find CARGO_MANIFEST_DIR"))?;

    let workspace_root = Path::new(&xtask_path).parent().unwrap();

    // Try to find the binary in target/debug or target/release
    let debug_binary = workspace_root.join("target/debug/codemod");
    let release_binary = workspace_root.join("target/release/codemod");

    let binary_path = if release_binary.exists() {
        release_binary
    } else if debug_binary.exists() {
        debug_binary
    } else {
        return Err(anyhow::anyhow!(
            "codemod binary not found. Please build it first with 'cargo build' or 'cargo build --release'"
        ));
    };

    // Execute codemod init with the specified arguments
    let mut cmd = Command::new(binary_path);
    cmd.arg("init");

    // If target_dir is specified, use it as the path and set it as current_dir
    // Otherwise, use "./" as the path
    if let Some(ref dir) = target_dir {
        cmd.arg(".").current_dir(dir);
    } else {
        cmd.arg("./");
    }

    cmd.arg("--name")
        .arg("my-codemod")
        .arg("--project-type")
        .arg("ast-grep-js")
        .arg("--package-manager")
        .arg("npm")
        .arg("--language")
        .arg("js")
        .arg("--author")
        .arg("Codemod")
        .arg("--description")
        .arg("migrations template")
        .arg("--license")
        .arg("MIT")
        .arg("--workspace")
        .arg("--github-action")
        .arg("--no-interactive")
        .arg("--force");

    let status = cmd.status()?;

    if !status.success() {
        return Err(anyhow::anyhow!(
            "codemod init failed with exit code: {:?}",
            status.code()
        ));
    }

    Ok(())
}
