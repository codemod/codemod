//! `codemod jssg test` follows `root.rename()` with a relative name and with
//! an absolute path built from `root.filename()`. The case runs in a temp
//! directory, which on macOS is `/var/...` canonicalized to `/private/var/...`:
//! both spellings must land on the expected file.

use std::fs;
use std::path::Path;
use std::process::Command;

const TRANSFORM: &str = r#"
export default function transform(root: any) {
  const name = root.filename();
  if (name.endsWith("abs/index.ts")) {
    root.rename(name.replace(/index\.ts$/, "index.barrel.bak.ts"));
    return root.root().text();
  }
  if (name.endsWith("rel/index.ts")) {
    root.rename("index.barrel.bak.ts");
    return root.root().text();
  }
  return null;
}
"#;

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn jssg_test_follows_absolute_and_relative_renames() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(&root.join("scripts/codemod.ts"), TRANSFORM);
    for kind in ["abs", "rel"] {
        let text = format!("export {{ a }} from \"./a\"; // {kind}\n");
        write(
            &root.join(format!("tests/case/input/src/{kind}/index.ts")),
            &text,
        );
        write(
            &root.join(format!("tests/case/input/src/{kind}/a.ts")),
            "export const a = 1;\n",
        );
        write(
            &root.join(format!(
                "tests/case/expected/src/{kind}/index.barrel.bak.ts"
            )),
            &text,
        );
        write(
            &root.join(format!("tests/case/expected/src/{kind}/a.ts")),
            "export const a = 1;\n",
        );
    }

    let run = Command::new(env!("CARGO_BIN_EXE_codemod"))
        .current_dir(root)
        .args([
            "--disable-analytics",
            "jssg",
            "test",
            "-l",
            "typescript",
            "scripts/codemod.ts",
            "tests",
        ])
        .output()
        .unwrap();
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(run.status.success(), "{output}");
    assert!(output.contains("6 passed"), "{output}");
}
