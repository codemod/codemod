use std::collections::HashMap;

use butterflow_models::Error;
use butterflow_runners::direct_runner::DirectRunner;
use butterflow_runners::Runner;

async fn run(command: &str) -> butterflow_models::Result<String> {
    let env: HashMap<String, String> = std::env::vars().collect();
    DirectRunner::with_quiet(true)
        .run_command(command, &env, None)
        .await
}

fn lines(output: &str) -> Vec<&str> {
    output.lines().map(str::trim_end).collect()
}

#[tokio::test]
async fn runs_every_line_of_a_multi_line_step() {
    let output = run("echo message1\necho message2\n").await.unwrap();
    assert_eq!(lines(&output), ["message1", "message2"]);
}

#[tokio::test]
async fn keeps_running_after_a_failing_line_and_reports_the_last_exit_code() {
    let output = run("echo before\nexit-is-not-a-command-butterflow\necho after")
        .await
        .unwrap();
    let output = lines(&output);
    assert_eq!(output.first(), Some(&"before"));
    assert_eq!(output.last(), Some(&"after"));

    let exit = if cfg!(windows) { "exit /b 3" } else { "exit 3" };
    match run(&format!("echo one\n{exit}")).await {
        Err(Error::ShellCommandFailed { exit_code, .. }) => assert_eq!(exit_code, 3),
        other => panic!("expected ShellCommandFailed, got {other:?}"),
    }
}

// cmd's echo prints the quotes it is given; sh's removes them. Either way the
// quoted string has to reach the shell as written (#1883).
#[tokio::test]
async fn passes_double_quoted_strings_through_unchanged() {
    let output = run("echo \"scripts.dev=pulse dev\"").await.unwrap();
    let expected = if cfg!(windows) {
        "\"scripts.dev=pulse dev\""
    } else {
        "scripts.dev=pulse dev"
    };
    assert_eq!(lines(&output), [expected]);
}

// A command that is a batch file, like npm (npm.cmd), returns to the step
// instead of ending it.
#[cfg(windows)]
#[tokio::test]
async fn continues_after_a_batch_file_command_on_windows() {
    let tools = std::env::temp_dir().join(format!("butterflow-tools-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tools).unwrap();
    std::fs::write(tools.join("faketool.cmd"), "@echo tool ran with [%*]\r\n").unwrap();

    let mut env: HashMap<String, String> = std::env::vars().collect();
    let path_key = env
        .keys()
        .find(|k| k.eq_ignore_ascii_case("PATH"))
        .cloned()
        .unwrap_or_else(|| "PATH".to_string());
    let path = format!(
        "{};{}",
        tools.display(),
        env.get(&path_key).cloned().unwrap_or_default()
    );
    env.insert(path_key, path);

    let output = DirectRunner::with_quiet(true)
        .run_command("faketool install\nfaketool build && echo after", &env, None)
        .await;
    std::fs::remove_dir_all(&tools).ok();

    assert_eq!(
        lines(&output.unwrap()),
        ["tool ran with [install]", "tool ran with [build]", "after"]
    );
}

// Non-ASCII text reaches cmd intact, as it did on the command line: a file
// created here is found by its name from the step.
#[cfg(windows)]
#[tokio::test]
async fn keeps_non_ascii_text_on_windows() {
    let path =
        std::env::temp_dir().join(format!("butterflow-café-日本-{}.txt", uuid::Uuid::new_v4()));
    std::fs::write(&path, "x").unwrap();
    let output = run(&format!(
        "if exist \"{}\" (echo found) else (echo missing)",
        path.display()
    ))
    .await;
    std::fs::remove_file(&path).ok();
    assert_eq!(lines(&output.unwrap()), ["found"]);
}
