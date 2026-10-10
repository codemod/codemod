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
