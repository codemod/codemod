//! `codemod jssg run` end to end, on the real binary.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

/// Appends `// touched by <file>` to every `// touch: <path>` the file names
/// (relative to the target), turns `OLD` into `NEW` in the file itself, and
/// throws after the touches when the file says `// throw`.
const CROSS_FILE: &str = r#"
import { jssgTransform } from "codemod:ast-grep";

export default async function transform(root: any, options: any) {
  const text: string = root.root().text();
  const name = root.filename().split(/[\\/]/).pop();
  for (const match of text.matchAll(/\/\/ touch: (\S+)/g)) {
    await jssgTransform(
      (r: any) => r.root().text() + `// touched by ${name}\n`,
      `${options.targetDir}/${match[1]}`,
      "typescript",
    );
  }
  if (text.includes("// throw")) throw new Error(`${name} fails on purpose`);
  const edit = text.replace("OLD", "NEW");
  return edit === text ? null : edit;
}
"#;

const FILES: usize = 24;

fn write(dir: &Path, path: &str, text: &str) {
    let full = dir.join(path);
    fs::create_dir_all(full.parent().unwrap()).unwrap();
    fs::write(full, text).unwrap();
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

struct Fixture {
    package: TempDir,
    target: TempDir,
}

impl Fixture {
    /// `FILES` files that each change `shared/index.ts`, plus `extra`.
    fn new(extra: &[(&str, &str)]) -> Self {
        let fixture = Self {
            package: TempDir::new().unwrap(),
            target: TempDir::new().unwrap(),
        };
        write(fixture.package.path(), "codemod.ts", CROSS_FILE);
        write(fixture.target.path(), "shared/index.ts", "export {};\n");
        for i in 0..FILES {
            write(
                fixture.target.path(),
                &format!("src/f{i:02}.ts"),
                &format!("// touch: shared/index.ts\nconst v{i} = 'OLD';\n"),
            );
        }
        for (path, text) in extra {
            write(fixture.target.path(), path, text);
        }
        fixture
    }

    fn read(&self, path: &str) -> String {
        fs::read_to_string(self.target.path().join(path)).unwrap()
    }

    fn run(&self, extra: &[&str]) -> Output {
        let script = self.package.path().join("codemod.ts");
        let mut args = vec![
            "jssg",
            "run",
            script.to_str().unwrap(),
            "--language",
            "typescript",
            "--target",
            self.target.path().to_str().unwrap(),
            "--allow-dirty",
            "--no-interactive",
            "--max-threads",
            "8",
        ];
        args.extend_from_slice(extra);
        Command::new(env!("CARGO_BIN_EXE_codemod"))
            .args(&args)
            .env("DISABLE_ANALYTICS", "true")
            .output()
            .expect("failed to run codemod")
    }

    /// The sorted lines of `shared/index.ts`.
    fn shared_lines(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .read("shared/index.ts")
            .lines()
            .map(str::to_string)
            .collect();
        lines.sort();
        lines
    }
}

fn touched_by_all() -> Vec<String> {
    let mut lines: Vec<String> = (0..FILES)
        .map(|i| format!("// touched by f{i:02}.ts"))
        .collect();
    lines.push("export {};".to_string());
    lines.sort();
    lines
}

/// Many files changing one shared file with `jssgTransform` all keep their
/// change, every time.
#[test]
fn jssg_run_keeps_every_files_change_to_a_shared_file() {
    for attempt in 0..5 {
        let fixture = Fixture::new(&[]);
        let output = fixture.run(&[]);
        assert!(output.status.success(), "{}", text(&output));
        assert_eq!(
            fixture.shared_lines(),
            touched_by_all(),
            "attempt {attempt}: {}",
            text(&output)
        );
        for i in 0..FILES {
            assert_eq!(
                fixture.read(&format!("src/f{i:02}.ts")),
                format!("// touch: shared/index.ts\nconst v{i} = 'NEW';\n")
            );
        }
    }
}

/// A file whose transform fails keeps none of its changes to other files;
/// the others' changes stay.
#[test]
fn jssg_run_drops_a_failed_files_changes_to_other_files() {
    let fixture = Fixture::new(&[(
        "src/bad.ts",
        "// touch: shared/index.ts\n// throw\nconst bad = 'OLD';\n",
    )]);
    let output = fixture.run(&[]);
    assert!(!output.status.success(), "{}", text(&output));
    assert!(text(&output).contains("bad.ts fails on purpose"));
    // Piped, so no color codes in the error.
    assert!(!text(&output).contains('\u{1b}'), "{}", text(&output));
    assert_eq!(fixture.shared_lines(), touched_by_all());
    assert_eq!(
        fixture.read("src/bad.ts"),
        "// touch: shared/index.ts\n// throw\nconst bad = 'OLD';\n"
    );
}

/// A file that can't be written fails the run, and says so once.
#[cfg(unix)]
#[test]
fn jssg_run_reports_a_write_error_once() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new(&[("src/ro.ts", "const ro = 'OLD';\n")]);
    let ro = fixture.target.path().join("src/ro.ts");
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o444)).unwrap();
    let output = fixture.run(&[]);
    assert!(!output.status.success(), "{}", text(&output));
    assert_eq!(
        text(&output)
            .matches("Failed to write modified file")
            .count(),
        1,
        "{}",
        text(&output)
    );
    assert_eq!(fixture.read("src/ro.ts"), "const ro = 'OLD';\n");
}

/// Without a terminal nothing is asked: a capability given with its flag is
/// used, and one that wasn't fails the run naming the flag.
#[test]
fn jssg_run_without_a_terminal_uses_the_capability_flags_given() {
    let fixture = Fixture::new(&[]);
    write(
        fixture.package.path(),
        "codemod.ts",
        r#"
import * as childProcess from "child_process";
export default function transform(root: any) {
  const text: string = root.root().text();
  const granted = typeof childProcess.spawn === "function";
  return granted && text.includes("OLD") ? text.replace("OLD", "NEW") : null;
}
"#,
    );
    let output = fixture.run(&[]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("pass --allow-child-process"),
        "{}",
        text(&output)
    );
    assert!(fixture.read("src/f00.ts").contains("'OLD'"));

    let output = fixture.run(&["--allow-child-process"]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        fixture.read("src/f00.ts").contains("'NEW'"),
        "{}",
        text(&output)
    );
}

/// A JavaScript error does not hide another file's error.
#[cfg(unix)]
#[test]
fn jssg_run_reports_every_files_error() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new(&[
        ("src/bad.ts", "// throw\nconst bad = 'OLD';\n"),
        ("src/ro.ts", "const ro = 'OLD';\n"),
    ]);
    let ro = fixture.target.path().join("src/ro.ts");
    fs::set_permissions(&ro, fs::Permissions::from_mode(0o444)).unwrap();
    let output = fixture.run(&[]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    let text = text(&output);
    assert!(text.contains("bad.ts fails on purpose"), "{text}");
    assert!(text.contains("Failed to write modified file"), "{text}");
    assert!(text.contains("jssg run failed with 2 errors"), "{text}");
}

/// A dry run writes nothing, and shows the shared file's diff with every
/// file's change.
#[test]
fn jssg_run_dry_run_shows_every_change_and_writes_nothing() {
    let fixture = Fixture::new(&[]);
    let output = fixture.run(&["--dry-run", "--no-color"]);
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(fixture.read("shared/index.ts"), "export {};\n");
    let out = text(&output);
    for i in 0..FILES {
        assert!(
            out.contains(&format!("+// touched by f{i:02}.ts")),
            "missing f{i:02}.ts in:\n{out}"
        );
    }
}

/// Files that change each other wait for each other's transaction instead
/// of losing a change or failing, however the threads interleave.
#[test]
fn jssg_run_files_changing_each_other_keep_both_changes() {
    for attempt in 0..5 {
        let pairs: Vec<(String, String)> = (0..8)
            .flat_map(|i| {
                [
                    (
                        format!("src/p{i}a.ts"),
                        format!("// touch: src/p{i}b.ts\nconst a = 'OLD';\n"),
                    ),
                    (
                        format!("src/p{i}b.ts"),
                        format!("// touch: src/p{i}a.ts\nconst b = 'OLD';\n"),
                    ),
                ]
            })
            .collect();
        let extra: Vec<(&str, &str)> = pairs
            .iter()
            .map(|(path, text)| (path.as_str(), text.as_str()))
            .collect();
        let fixture = Fixture::new(&extra);
        let output = fixture.run(&[]);
        assert!(
            output.status.success(),
            "attempt {attempt}: {}",
            text(&output)
        );
        for i in 0..8 {
            assert_eq!(
                fixture.read(&format!("src/p{i}a.ts")),
                format!("// touch: src/p{i}b.ts\nconst a = 'NEW';\n// touched by p{i}b.ts\n"),
                "attempt {attempt}"
            );
            assert_eq!(
                fixture.read(&format!("src/p{i}b.ts")),
                format!("// touch: src/p{i}a.ts\nconst b = 'NEW';\n// touched by p{i}a.ts\n"),
                "attempt {attempt}"
            );
        }
        assert_eq!(fixture.shared_lines(), touched_by_all());
    }
}

/// A transform that writes another file with `root.write()` starts from that
/// file's text after its own transform, and the run keeps both changes.
#[test]
fn jssg_run_keeps_a_files_change_and_a_later_root_write_to_it() {
    let package = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    write(
        package.path(),
        "codemod.js",
        r#"
export default function transform(root) {
  const text = root.root().text();
  if (text.includes("import")) {
    const spec = root.root().find({ rule: { kind: "import_specifier" } });
    const def = spec.field("name").definition();
    if (!def || def.kind !== "external") throw new Error("no definition");
    def.root.write(def.root.root().text() + "// written by b_user\n");
    return null;
  }
  const edit = text.replace("OLD", "NEW");
  return edit === text ? null : edit;
}
"#,
    );
    write(target.path(), "a_lib.ts", "export const x = 'OLD';\n");
    write(
        target.path(),
        "b_user.ts",
        "import { x } from \"./a_lib\";\nconsole.log(x);\n",
    );
    let script = package.path().join("codemod.js");
    let output = Command::new(env!("CARGO_BIN_EXE_codemod"))
        .args([
            "jssg",
            "run",
            script.to_str().unwrap(),
            "--language",
            "typescript",
            "--target",
            target.path().to_str().unwrap(),
            "--semantic-workspace",
            target.path().to_str().unwrap(),
            "--allow-dirty",
            "--no-interactive",
            "--max-threads",
            "1",
        ])
        .env("DISABLE_ANALYTICS", "true")
        .output()
        .expect("failed to run codemod");
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(
        fs::read_to_string(target.path().join("a_lib.ts")).unwrap(),
        "export const x = 'NEW';\n// written by b_user\n",
        "{}",
        text(&output)
    );
}

/// Renaming a symlink with `jssgTransform` moves the link: the file it
/// points to stays.
#[cfg(unix)]
#[test]
fn jssg_run_renaming_a_symlink_keeps_its_target() {
    let package = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    write(
        package.path(),
        "codemod.ts",
        r#"
import { jssgTransform } from "codemod:ast-grep";

export default async function transform(root: any, options: any) {
  if (!root.filename().endsWith("main.ts")) return null;
  await jssgTransform(
    (r: any) => {
      r.rename("renamed.ts");
      return r.root().text().replace("OLD", "NEW");
    },
    `${options.targetDir}/link.ts`,
    "typescript",
  );
  return null;
}
"#,
    );
    write(target.path(), "main.ts", "export {};\n");
    write(target.path(), "real.ts", "const x = 'OLD';\n");
    std::os::unix::fs::symlink(target.path().join("real.ts"), target.path().join("link.ts"))
        .unwrap();
    let script = package.path().join("codemod.ts");
    let output = Command::new(env!("CARGO_BIN_EXE_codemod"))
        .args([
            "jssg",
            "run",
            script.to_str().unwrap(),
            "--language",
            "typescript",
            "--target",
            target.path().to_str().unwrap(),
            "--allow-dirty",
            "--no-interactive",
        ])
        .env("DISABLE_ANALYTICS", "true")
        .output()
        .expect("failed to run codemod");
    assert!(output.status.success(), "{}", text(&output));
    assert!(target.path().join("real.ts").exists(), "{}", text(&output));
    assert!(!target.path().join("link.ts").exists(), "{}", text(&output));
    assert_eq!(
        fs::read_to_string(target.path().join("renamed.ts")).unwrap(),
        "const x = 'NEW';\n"
    );
}
