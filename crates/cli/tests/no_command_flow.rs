use std::process::Command;

fn codemod_binary() -> &'static str {
    env!("CARGO_BIN_EXE_codemod")
}

#[test]
fn help_flag_keeps_standard_help_output() {
    let output = Command::new(codemod_binary())
        .arg("--help")
        .output()
        .expect("failed to run codemod --help");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(stdout.contains("Usage: codemod [OPTIONS] [COMMAND]"));
    assert!(stdout.contains("Commands:"));
    assert!(!stdout.contains("What would you like to do?"));
    assert!(stderr.is_empty());
}

#[test]
fn no_command_in_non_interactive_mode_prints_next_steps() {
    let output = Command::new(codemod_binary())
        .output()
        .expect("failed to run codemod without arguments");

    assert_eq!(output.status.code(), Some(1));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(stdout.matches("      __                  __").count(), 1);
    assert!(stderr.contains("No command provided."));
    // Pinned to this version: a bare `npx codemod` is 1.x while 2.x is a prerelease.
    let npx = format!("npx codemod@{}", env!("CARGO_PKG_VERSION"));
    assert!(stderr.contains(&format!("1. Install Master Codemod Skills: {npx} ai")));
    assert!(stderr.contains(&format!("2. Create a new codemod package: {npx} init")));
    assert!(stderr.contains(&format!("3. Run a published package: {npx} <package>")));
    assert!(!stdout.contains("What would you like to do?"));
}

#[test]
fn init_without_a_terminal_does_not_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(codemod_binary())
        .arg("init")
        .current_dir(dir.path())
        .env("DISABLE_ANALYTICS", "true")
        .output()
        .expect("failed to run codemod init");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Treated as --no-interactive: a missing flag, not a failed prompt.
    assert!(stderr.contains("Path argument is required"), "{stderr}");
    assert!(!stderr.contains("not a TTY"), "{stderr}");
    assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none());
}
