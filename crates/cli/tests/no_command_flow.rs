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
    assert!(stderr.contains("1. Install Master Codemod Skills: npx codemod ai"));
    assert!(stderr.contains("2. Create a new codemod package: npx codemod init"));
    assert!(stderr.contains("3. Run a published package: npx codemod <package>"));
    assert!(!stdout.contains("What would you like to do?"));
}
