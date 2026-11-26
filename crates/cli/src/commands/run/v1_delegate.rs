//! `codemod run --v1 <pkg>`: run a 1.x package with codemod 1.x. Opt-in and
//! deprecated; to be removed in a future release.
//!
//! By default 1.x packages run on this version (through the compatibility
//! modules). Only when the user asks, with `--v1` or `CODEMOD_V1=on` (for CI),
//! is the package handed to a pinned codemod 1.x binary instead.
//!
//! # Which packages are 1.x
//!
//! A package can run with codemod 1.x when both hold:
//!
//! 1. the workflow `codemod run` would run is a `.yaml`/`.yml` file. codemod
//!    1.x only ever ran `workflow.yaml`, and a package with a `workflow.ts`
//!    can't be a 1.x package. The manifest's `schema_version` is `1.0` in both
//!    versions, so it says nothing;
//! 2. no JavaScript/TypeScript file in the package imports a module with the
//!    2.0-only spelling `codemod/<name>` (`codemod/ast-grep`, ...). 1.x knows
//!    only `codemod:<name>`, so a YAML package that uses the new spelling was
//!    written for 2.0.
//!
//! With `--v1`, a package that isn't 1.x, or a run that also asks for
//! `--control`, is an error. With only `CODEMOD_V1=on` (set for a whole CI
//! job), a package that isn't 1.x just runs on this version.

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use sha2::{Digest, Sha512};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

/// The codemod 1.x release 1.x packages run with.
pub const V1_VERSION: &str = "1.18.7";

/// npm `dist.integrity` of each platform package of [`V1_VERSION`], so a
/// download is checked against what this binary was built with, not against
/// what the registry says today.
const V1_PLATFORMS: &[(&str, &str)] = &[
    ("darwin-arm64", "sha512-ZxvZOk8dTqoL5/MB5BcXccrJGavvlv44mZnoOvvbKtoyFEDwJlMcE0SDsLQ6/1WiEPUaBxfIUMqlPlon0EvLJA=="),
    ("darwin-x64", "sha512-yaUFBYiRtX8l/+U1/061xHjFsmxMJgCNVCdixg91LBp+t0Znbh02AQkH5iBV5uK/VsljUwd3Z680X43YIbed8w=="),
    ("linux-x64-gnu", "sha512-KrmyOe+NlzAj/E4qn7YpKF/bm4CbDbshRBI3upPNGXWLAxSVuy/zIMVic9KJcfgnNRqHNhKzzGKSBvGJkyzgpA=="),
    ("linux-x64-musl", "sha512-e7JhVMyJde+Ko4rCWvggcCOCGZy2dYSxs7QI/xtdBs5t9DZD2UuJLoJM+WX+7UiqRDOGppjCSY3+CT0hpcRTTA=="),
    ("linux-arm64-gnu", "sha512-hvR3eYmRZLbjz/+/rmxhOTE5cRAxAqFTqItiHXEJQTHSkPVKupD3tccLmRl0pfxrVf4aBGhUwZ8gpGVVSbsfcw=="),
    ("linux-arm64-musl", "sha512-sqyFtpjfBYONpprA9V+AEPnK5iLm8aL9XddOL6rYWFKVgHQZa1reaVt/cZ2uiM+CDTn2YKAurVX54rkRaL+bqw=="),
    ("win32-x64-msvc", "sha512-DnKzYRDXah1q3qH2WOST47dLndapje3qAqc6lI6RJwPw4oZhrM7lBBKWbDkMRJPKe3rtf6q1cYT3m/CsypB5Jw=="),
];

/// Printed (dim, on stderr) on every run handed to codemod 1.x.
pub const DEPRECATION_NOTE: &str =
    "--v1 runs codemod 1.18.7 and will be removed in a future release.";

/// The hint for an error that refuses to run a 1.x package on this version.
/// `{package}` is replaced with what the user ran.
///
/// TODO(v1-hint): the compatibility path (T-79) should end its refusal
/// message (docker/podman steps, transforms that need real side effects, ...)
/// with `v1_hint(package)`. Nothing calls it yet; wire it where that refusal
/// is built.
pub const V1_HINT: &str =
    "To run it with codemod 1.18.7 instead (deprecated): codemod run --v1 {package}";

/// [`V1_HINT`] for a package.
pub fn v1_hint(package: &str) -> String {
    V1_HINT.replace("{package}", package)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Engine {
    /// Run on this version.
    V2,
    /// Run with codemod 1.x.
    V1,
}

/// What the run asked for that decides the engine, besides the package.
#[derive(Debug, Clone, Default)]
pub struct RunAsks {
    /// `--v1`.
    pub flag: bool,
    pub control: bool,
    /// The value of `CODEMOD_V1`.
    pub env: Option<String>,
}

impl RunAsks {
    fn env_on(&self) -> bool {
        self.env.as_deref().is_some_and(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "on" | "1" | "true" | "yes"
            )
        })
    }
}

/// Decides which engine runs a package, by the rule in the module docs.
/// Never picks codemod 1.x unless `--v1` or `CODEMOD_V1=on` asked for it.
pub fn detect(package_dir: &Path, workflow: &Path, asks: &RunAsks) -> Result<Engine> {
    if !asks.flag && !asks.env_on() {
        return Ok(Engine::V2);
    }
    let asked_by = if asks.flag { "--v1" } else { "CODEMOD_V1=on" };
    if asks.control {
        bail!("{asked_by} can't be combined with --control: codemod 1.x has no control channel");
    }
    match why_not_v1(package_dir, workflow) {
        None => Ok(Engine::V1),
        // CI may set CODEMOD_V1=on for every run; packages written for this
        // version just run on it.
        Some(_) if !asks.flag => Ok(Engine::V2),
        Some(why) => bail!("--v1 only runs packages written for codemod 1.x: {why}"),
    }
}

/// Why a package isn't a 1.x package, or `None` when it is.
fn why_not_v1(package_dir: &Path, workflow: &Path) -> Option<String> {
    let is_yaml = workflow
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("yaml") || e.eq_ignore_ascii_case("yml"));
    if !is_yaml {
        return Some(format!(
            "its workflow is {}, and codemod 1.x only runs workflow.yaml",
            workflow
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        ));
    }
    let file = first_v2_import(package_dir)?;
    Some(format!(
        "{} imports a `codemod/<name>` module, which codemod 1.x doesn't have",
        file.strip_prefix(package_dir).unwrap_or(&file).display()
    ))
}

/// The first script in the package that imports a `codemod/<name>` module.
fn first_v2_import(package_dir: &Path) -> Option<PathBuf> {
    let mut stack = vec![package_dir.to_path_buf()];
    let mut seen = 0usize;
    while let Some(dir) = stack.pop() {
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
                if !matches!(name.as_ref(), "node_modules" | ".git" | "dist" | "target")
                    && !name.starts_with('.')
                {
                    stack.push(path);
                }
                continue;
            }
            let is_script = matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("js" | "mjs" | "cjs" | "ts" | "mts" | "cts" | "jsx" | "tsx")
            );
            if !is_script {
                continue;
            }
            seen += 1;
            if seen > 2000 {
                return None;
            }
            if let Ok(text) = std::fs::read_to_string(&path) {
                if imports_v2_module(&text) {
                    return Some(path);
                }
            }
        }
    }
    None
}

/// Whether source text imports a module spelled `codemod/<name>`.
pub fn imports_v2_module(text: &str) -> bool {
    ["\"codemod/", "'codemod/", "`codemod/"]
        .iter()
        .any(|quote| {
            text.match_indices(quote).any(|(at, _)| {
                let before = text[..at].trim_end();
                before.ends_with("from")
                    || before.ends_with("import")
                    || before.ends_with("import(")
                    || before.ends_with("require(")
            })
        })
}

/// Options of `codemod run` passed on to codemod 1.x.
#[derive(Debug, Clone, Default)]
pub struct Passthrough {
    pub package: String,
    pub registry: Option<String>,
    pub force: bool,
    pub dry_run: bool,
    pub params: Vec<String>,
    pub allow_dirty: bool,
    pub target: Option<PathBuf>,
    pub no_color: bool,
    pub report: bool,
    pub llm_usage_output: Option<PathBuf>,
    pub workflow: Option<String>,
    pub allow_fs: bool,
    pub allow_fetch: bool,
    pub allow_child_process: bool,
    pub no_interactive: bool,
    pub agent: Option<String>,
    pub install_skill: bool,
    pub jsonl: bool,
    pub verbose: bool,
    pub disable_analytics: bool,
}

/// The arguments for `codemod run` of codemod 1.x. Flags 1.x doesn't have
/// (`--approve`, `--managed-git`, `--allow-ai`, `--model`) are not passed.
pub fn v1_args(p: &Passthrough) -> Vec<String> {
    let mut args = vec!["run".to_string(), p.package.clone()];
    let mut flag = |on: bool, name: &str| {
        if on {
            args.push(name.to_string());
        }
    };
    flag(p.force, "--force");
    flag(p.dry_run, "--dry-run");
    flag(p.allow_dirty, "--allow-dirty");
    flag(p.no_color, "--no-color");
    flag(p.report, "--report");
    flag(p.allow_fs, "--allow-fs");
    flag(p.allow_fetch, "--allow-fetch");
    flag(p.allow_child_process, "--allow-child-process");
    flag(p.no_interactive, "--no-interactive");
    flag(p.install_skill, "--install-skill");
    flag(p.verbose, "--verbose");
    flag(p.disable_analytics, "--disable-analytics");
    if let Some(registry) = &p.registry {
        args.extend(["--registry".into(), registry.clone()]);
    }
    for param in &p.params {
        args.extend(["--param".into(), param.clone()]);
    }
    if let Some(target) = &p.target {
        args.extend(["--target".into(), target.to_string_lossy().into_owned()]);
    }
    if let Some(path) = &p.llm_usage_output {
        args.extend([
            "--llm-usage-output".into(),
            path.to_string_lossy().into_owned(),
        ]);
    }
    if let Some(name) = &p.workflow {
        args.extend(["--workflow".into(), name.clone()]);
    }
    if let Some(agent) = &p.agent {
        args.extend(["--agent".into(), agent.clone()]);
    }
    if p.jsonl {
        args.extend(["--format".into(), "jsonl".into()]);
    }
    args
}

/// The npm platform package name suffix for this machine, as the npm wrapper
/// picks it. The libc follows the one this binary was built for.
fn platform() -> Option<&'static str> {
    let musl = cfg!(target_env = "musl");
    Some(match (std::env::consts::OS, std::env::consts::ARCH, musl) {
        ("macos", "aarch64", _) => "darwin-arm64",
        ("macos", "x86_64", _) => "darwin-x64",
        ("linux", "x86_64", false) => "linux-x64-gnu",
        ("linux", "x86_64", true) => "linux-x64-musl",
        ("linux", "aarch64", false) => "linux-arm64-gnu",
        ("linux", "aarch64", true) => "linux-arm64-musl",
        ("windows", "x86_64", _) => "win32-x64-msvc",
        _ => return None,
    })
}

fn binary_name() -> &'static str {
    if cfg!(windows) {
        "codemod.exe"
    } else {
        "codemod"
    }
}

/// Where the pinned 1.x binary is kept.
pub fn cache_path() -> Result<PathBuf> {
    let base = dirs::data_dir().ok_or_else(|| anyhow!("Could not find the data directory"))?;
    Ok(base
        .join("codemod")
        .join("cache")
        .join("cli-v1")
        .join(V1_VERSION)
        .join(binary_name()))
}

/// The codemod 1.x binary to run: `CODEMOD_V1_BIN`, else the cached one,
/// else one downloaded now from npm and checked against the pinned hash.
pub async fn resolve_binary() -> Result<(PathBuf, bool)> {
    if let Some(bin) = std::env::var_os("CODEMOD_V1_BIN") {
        return Ok((PathBuf::from(bin), false));
    }
    let path = cache_path()?;
    if path.is_file() {
        return Ok((path, false));
    }
    let platform = platform().ok_or_else(|| {
        anyhow!(
            "codemod {V1_VERSION} has no build for {}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    let integrity = V1_PLATFORMS
        .iter()
        .find(|(p, _)| *p == platform)
        .map(|(_, i)| *i)
        .expect("every platform has a hash");
    let url = format!(
        "https://registry.npmjs.org/@codemod.com/cli-{platform}/-/cli-{platform}-{V1_VERSION}.tgz"
    );
    let bytes = reqwest::get(&url)
        .await
        .and_then(|r| r.error_for_status())
        .with_context(|| format!("Could not download codemod {V1_VERSION} from {url}"))?
        .bytes()
        .await
        .with_context(|| format!("Could not download codemod {V1_VERSION} from {url}"))?;
    check_integrity(&bytes, integrity)?;
    let binary = extract_binary(&bytes)?;
    let dir = path.parent().expect("cache path has a parent");
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.{}", binary_name(), std::process::id()));
    std::fs::write(&tmp, binary)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&tmp, &path)?;
    Ok((path, true))
}

/// Checks bytes against an npm `sha512-<base64>` integrity string.
pub fn check_integrity(bytes: &[u8], integrity: &str) -> Result<()> {
    let expected = integrity
        .strip_prefix("sha512-")
        .ok_or_else(|| anyhow!("unsupported integrity {integrity}"))?;
    let actual = base64::engine::general_purpose::STANDARD.encode(Sha512::digest(bytes));
    if actual != expected {
        bail!("The codemod {V1_VERSION} download doesn't match its pinned hash; not running it");
    }
    Ok(())
}

/// The `package/codemod[.exe]` file of an npm platform package tarball.
fn extract_binary(tgz: &[u8]) -> Result<Vec<u8>> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(tgz));
    let wanted = format!("package/{}", binary_name());
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.path()?.to_string_lossy() == wanted {
            let mut out = Vec::new();
            entry.read_to_end(&mut out)?;
            return Ok(out);
        }
    }
    bail!("The codemod {V1_VERSION} package has no {wanted}")
}

/// Runs codemod 1.x with the terminal handed over, and returns its exit code.
pub fn run(binary: &Path, args: &[String]) -> Result<i32> {
    let status = ProcessCommand::new(binary)
        .args(args)
        // Keep 1.x from delegating back or reading 2.0-only settings.
        .env_remove("CODEMOD_V1")
        .status()
        .with_context(|| {
            format!(
                "Could not start codemod {V1_VERSION} ({})",
                binary.display()
            )
        })?;
    Ok(exit_code(status))
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in files {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dir
    }

    const V1_PACKAGE: &[(&str, &str)] = &[
        ("workflow.yaml", "version: \"1\"\nnodes: []\n"),
        (
            "scripts/codemod.ts",
            "import type { Transform } from \"codemod:ast-grep\";\n",
        ),
    ];

    fn flag() -> RunAsks {
        RunAsks {
            flag: true,
            ..Default::default()
        }
    }

    fn env(value: &str) -> RunAsks {
        RunAsks {
            env: Some(value.into()),
            ..Default::default()
        }
    }

    fn engine(files: &[(&str, &str)], workflow: &str, asks: RunAsks) -> Result<Engine> {
        let dir = package(files);
        detect(dir.path(), &dir.path().join(workflow), &asks)
    }

    #[test]
    fn never_delegates_without_asking() {
        let e = engine(V1_PACKAGE, "workflow.yaml", RunAsks::default()).unwrap();
        assert_eq!(e, Engine::V2);
        for off in ["off", "0", "", "maybe"] {
            assert_eq!(
                engine(V1_PACKAGE, "workflow.yaml", env(off)).unwrap(),
                Engine::V2
            );
        }
    }

    #[test]
    fn flag_runs_1x_package_on_v1() {
        assert_eq!(
            engine(V1_PACKAGE, "workflow.yaml", flag()).unwrap(),
            Engine::V1
        );
        assert_eq!(
            engine(&[("wf.yml", "")], "wf.yml", flag()).unwrap(),
            Engine::V1
        );
    }

    #[test]
    fn env_on_runs_1x_package_on_v1() {
        for on in ["on", "1", "true", "ON"] {
            assert_eq!(
                engine(V1_PACKAGE, "workflow.yaml", env(on)).unwrap(),
                Engine::V1
            );
        }
    }

    #[test]
    fn flag_on_typescript_workflow_is_an_error() {
        let err = engine(
            &[("workflow.ts", ""), ("workflow.yaml", "")],
            "workflow.ts",
            flag(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("workflow.ts"), "{err}");
    }

    #[test]
    fn flag_on_yaml_with_2x_imports_is_an_error() {
        let err = engine(
            &[
                ("workflow.yaml", ""),
                (
                    "scripts/codemod.ts",
                    "import { parse } from 'codemod/ast-grep';\n",
                ),
            ],
            "workflow.yaml",
            flag(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("scripts/codemod.ts"), "{err}");
    }

    #[test]
    fn env_on_leaves_2x_packages_on_v2() {
        let e = engine(&[("workflow.ts", "")], "workflow.ts", env("on")).unwrap();
        assert_eq!(e, Engine::V2);
    }

    #[test]
    fn node_modules_are_not_scanned() {
        let e = engine(
            &[
                ("workflow.yaml", ""),
                (
                    "node_modules/x/index.js",
                    "import { parse } from 'codemod/ast-grep';\n",
                ),
            ],
            "workflow.yaml",
            flag(),
        )
        .unwrap();
        assert_eq!(e, Engine::V1);
    }

    #[test]
    fn control_with_v1_is_an_error() {
        for mut asks in [flag(), env("on")] {
            asks.control = true;
            let err = engine(V1_PACKAGE, "workflow.yaml", asks).unwrap_err();
            assert!(err.to_string().contains("--control"), "{err}");
        }
        let asks = RunAsks {
            control: true,
            ..Default::default()
        };
        assert_eq!(
            engine(V1_PACKAGE, "workflow.yaml", asks).unwrap(),
            Engine::V2
        );
    }

    #[test]
    fn hint_names_the_package() {
        assert_eq!(
            v1_hint("@scope/pkg"),
            "To run it with codemod 1.18.7 instead (deprecated): codemod run --v1 @scope/pkg"
        );
        assert!(V1_HINT.contains(V1_VERSION));
        assert!(DEPRECATION_NOTE.contains(V1_VERSION));
    }

    #[test]
    fn import_spelling() {
        assert!(imports_v2_module("import x from \"codemod/workflow\""));
        assert!(imports_v2_module("import \"codemod/register\""));
        assert!(imports_v2_module("await import('codemod/ast-grep')"));
        assert!(imports_v2_module("const a = require(`codemod/x`)"));
        assert!(!imports_v2_module("import x from \"codemod:ast-grep\""));
        assert!(!imports_v2_module("// see \"codemod/workflow\" docs"));
        assert!(!imports_v2_module("import x from \"@codemod/utils\""));
    }

    #[test]
    fn args_map_flags_1x_has() {
        let args = v1_args(&Passthrough {
            package: "@scope/pkg@1.2.3".into(),
            dry_run: true,
            params: vec!["a=1".into(), "b=x".into()],
            target: Some(PathBuf::from("/repo")),
            no_interactive: true,
            jsonl: true,
            disable_analytics: true,
            ..Default::default()
        });
        assert_eq!(
            args,
            [
                "run",
                "@scope/pkg@1.2.3",
                "--dry-run",
                "--no-interactive",
                "--disable-analytics",
                "--param",
                "a=1",
                "--param",
                "b=x",
                "--target",
                "/repo",
                "--format",
                "jsonl"
            ]
        );
    }

    #[test]
    fn integrity_is_checked() {
        let good = format!(
            "sha512-{}",
            base64::engine::general_purpose::STANDARD.encode(Sha512::digest(b"hi"))
        );
        check_integrity(b"hi", &good).unwrap();
        assert!(check_integrity(b"ho", &good).is_err());
    }

    #[test]
    fn every_platform_is_pinned() {
        if let Some(p) = platform() {
            assert!(V1_PLATFORMS.iter().any(|(q, _)| *q == p));
        }
    }
}
