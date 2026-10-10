// Runs in its own test binary: it points the temporary directory, where
// Windows steps are written, at a path that cmd would split if unquoted.
#![cfg(windows)]

use std::collections::HashMap;

use butterflow_runners::direct_runner::DirectRunner;
use butterflow_runners::Runner;

#[tokio::test]
async fn runs_from_a_temp_dir_with_cmd_metacharacters() {
    let temp = std::env::temp_dir().join(format!("a&b^c {}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp).unwrap();
    std::env::set_var("TMP", &temp);
    std::env::set_var("TEMP", &temp);

    let env: HashMap<String, String> = std::env::vars().collect();
    let output = DirectRunner::with_quiet(true)
        .run_command("echo ok", &env, None)
        .await;

    let left_behind = std::fs::read_dir(&temp).unwrap().count();
    std::fs::remove_dir_all(&temp).ok();

    assert_eq!(output.unwrap().trim_end(), "ok");
    assert_eq!(left_behind, 0, "the temporary script is removed");
}
