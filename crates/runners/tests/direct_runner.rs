use std::collections::HashMap;

use butterflow_runners::direct_runner::{CommandOutput, DirectRunner};
use butterflow_runners::Runner;

async fn run(command: &str) -> butterflow_models::Result<CommandOutput> {
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
    assert_eq!(lines(&output.stdout), ["message1", "message2"]);
}

#[tokio::test]
async fn keeps_running_after_a_failing_line_and_reports_the_last_exit_code() {
    let output = run("echo before\nexit-is-not-a-command-butterflow\necho after")
        .await
        .unwrap();
    let output = lines(&output.stdout);
    assert_eq!(output.first(), Some(&"before"));
    assert_eq!(output.last(), Some(&"after"));

    let exit = if cfg!(windows) { "exit /b 3" } else { "exit 3" };
    let output = run(&format!("echo one\n{exit}")).await.unwrap();
    assert_eq!(output.exit_code, 3);
}

#[cfg(unix)]
#[tokio::test]
async fn keeps_stdout_and_stderr_apart() {
    let output = run("echo out\necho err >&2").await.unwrap();
    assert_eq!(
        (output.stdout.as_str(), output.stderr.as_str()),
        ("out\n", "err\n")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn captures_original_line_endings_and_logs_each_line() {
    use std::sync::{Arc, Mutex};

    let env = std::env::vars().collect();
    let logs = Arc::new(Mutex::new(Vec::new()));
    let output = DirectRunner::with_quiet(true)
        .run_command(
            r"printf 'first\r\n\nbad\377 byte\r\nlast\r'; printf 'bad\376\nfinal' >&2",
            &env,
            Some(Arc::new({
                let logs = Arc::clone(&logs);
                move |message| logs.lock().unwrap().push(message)
            })),
        )
        .await
        .unwrap();

    assert_eq!(output.exit_code, 0);
    assert!(!output.cut);
    assert_eq!(output.stdout, "first\r\n\nbad\u{fffd} byte\r\nlast\r");
    assert_eq!(output.stderr, "bad\u{fffd}\nfinal");

    let logs = logs.lock().unwrap();
    let stream_logs = |tag| {
        logs.iter()
            .filter(|line| line.starts_with(tag))
            .map(String::as_str)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        stream_logs("[stdout] "),
        [
            "[stdout] first",
            "[stdout] ",
            "[stdout] bad\u{fffd} byte",
            "[stdout] last\r"
        ]
    );
    assert_eq!(
        stream_logs("[stderr] "),
        ["[stderr] bad\u{fffd}", "[stderr] final"]
    );
}

#[cfg(windows)]
#[tokio::test]
async fn preserves_cmd_crlf_line_endings() {
    let output = run("echo out\n(echo err)1>&2").await.unwrap();
    assert_eq!(output.stdout, "out\r\n");
    assert_eq!(output.stderr, "err\r\n");
    assert!(!output.cut);
}

#[cfg(unix)]
#[tokio::test]
async fn keeps_the_end_of_output_that_is_too_long() {
    let output = run("yes line | head -n 400000\necho last").await.unwrap();
    let all = format!("{}last\n", "line\n".repeat(400_000));
    let kept = butterflow_runners::direct_runner::KEPT_OUTPUT;
    assert!(output.cut);
    assert_eq!(output.stdout, all[all.len() - kept..]);
}

#[cfg(unix)]
#[tokio::test]
async fn keeps_the_last_mib_when_a_long_line_is_followed_by_a_short_one() {
    let output = run("head -c 1048575 /dev/zero | tr '\\0' a; echo; echo X")
        .await
        .unwrap();
    assert_eq!(
        output.stdout.len(),
        butterflow_runners::direct_runner::KEPT_OUTPUT
    );
    assert!(output.stdout.ends_with("aa\nX\n"));
}

#[cfg(unix)]
#[tokio::test]
async fn keeps_the_end_of_a_single_line_that_is_too_long() {
    let output = run("head -c 2000000 /dev/zero | tr '\\0' a; echo b")
        .await
        .unwrap();
    assert!(output.cut);
    assert_eq!(
        output.stdout.len(),
        butterflow_runners::direct_runner::KEPT_OUTPUT
    );
    assert!(output.stdout.ends_with("aab\n"));
}

#[cfg(unix)]
#[tokio::test]
async fn cuts_an_unterminated_line_at_a_utf8_boundary() {
    use butterflow_runners::direct_runner::KEPT_OUTPUT;

    let command = format!(
        r"printf '\303\251'; head -c {} /dev/zero | tr '\0' a",
        KEPT_OUTPUT - 1
    );
    let output = run(&command).await.unwrap();

    assert!(output.cut);
    assert_eq!(output.stdout, "a".repeat(KEPT_OUTPUT - 1));
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
    assert_eq!(lines(&output.stdout), [expected]);
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
        lines(&output.unwrap().stdout),
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
    assert_eq!(lines(&output.unwrap().stdout), ["found"]);
}

#[cfg(unix)]
#[tokio::test]
async fn a_line_that_is_not_utf8_keeps_the_rest_of_the_output() {
    let output = run(r#"printf 'a\n\377\n{"ok":1}\n'"#).await.unwrap();
    assert_eq!(output.stdout, "a\n\u{FFFD}\n{\"ok\":1}\n");
}

#[cfg(unix)]
#[tokio::test]
async fn a_script_whose_interpreter_follows_a_space_runs_with_that_interpreter() {
    let output = run("#! /bin/cat\nnot a shell command\n").await.unwrap();
    assert_eq!(output.stdout, "#! /bin/cat\nnot a shell command\n");
}
