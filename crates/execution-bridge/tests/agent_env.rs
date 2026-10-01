//! The agent's API key handling in a bridge process. One test in its own
//! binary: it mutates the process environment, which must not race with
//! other tests.

use butterflow_execution_bridge::agent;

#[test]
fn process_settings_take_the_key_and_children_do_not_inherit_it() {
    // Environment channel (other launchers).
    std::env::remove_var(agent::SECRETS_ENV);
    std::env::set_var(agent::API_KEY_ENV, "from-env");
    let settings = agent::take_process_settings(std::io::empty()).expect("settings");
    assert_eq!(settings.api_key, "from-env");
    assert!(std::env::var_os(agent::API_KEY_ENV).is_none());
    #[cfg(unix)]
    {
        let child = std::process::Command::new("sh")
            .arg("-c")
            .arg("printf '%s/%s' \"${LLM_API_KEY-unset}\" \"${CODEMOD_BRIDGE_SECRETS-unset}\"")
            .output()
            .expect("sh runs");
        assert_eq!(String::from_utf8_lossy(&child.stdout), "unset/unset");
    }

    // Stdin channel (the TypeScript host): the key never was in the environment.
    std::env::set_var(agent::SECRETS_ENV, "stdin");
    let settings =
        agent::take_process_settings(&br#"{"LLM_API_KEY":"from-stdin"}"#[..]).expect("settings");
    assert_eq!(settings.api_key, "from-stdin");
    assert!(std::env::var_os(agent::SECRETS_ENV).is_none());

    // Stdin wins over a key that is also in the environment; both are removed.
    std::env::set_var(agent::SECRETS_ENV, "stdin");
    std::env::set_var(agent::API_KEY_ENV, "from-env");
    let settings =
        agent::take_process_settings(&br#"{"LLM_API_KEY":"from-stdin"}"#[..]).expect("settings");
    assert_eq!(settings.api_key, "from-stdin");
    assert!(std::env::var_os(agent::API_KEY_ENV).is_none());

    // Malformed channels fail as configuration errors and still clean up.
    for (marker, stdin, message) in [
        (
            "stdin",
            &b"not json"[..],
            "agent secrets on stdin must be a JSON object of strings",
        ),
        (
            "stdin",
            &br#"{"AWS_SECRET":"x"}"#[..],
            "unsupported agent secret 'AWS_SECRET'",
        ),
        (
            "file",
            &b""[..],
            "unsupported CODEMOD_BRIDGE_SECRETS value 'file'",
        ),
        ("stdin", &b"{}"[..], "agent requires LLM_API_KEY"),
    ] {
        std::env::set_var(agent::SECRETS_ENV, marker);
        std::env::set_var(agent::API_KEY_ENV, "");
        assert_eq!(
            agent::take_process_settings(stdin),
            Err(message.to_string())
        );
        assert!(std::env::var_os(agent::SECRETS_ENV).is_none());
        assert!(std::env::var_os(agent::API_KEY_ENV).is_none());
    }

    // External backends: the library path leaves `LLM_API_KEY` in the process
    // environment, yet the CLI process is started without it.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let bin = tempfile::tempdir().expect("bin");
        let repo = tempfile::tempdir().expect("repo");
        let fake = bin.path().join("claude");
        std::fs::write(
            &fake,
            r#"#!/bin/sh
if [ "$1" = auth ]; then echo '{"loggedIn":true}'; exit 0; fi
cat > /dev/null
printf '{"type":"result","subtype":"success","is_error":false,"result":"%s/%s"}\n' "${LLM_API_KEY-unset}" "${CODEMOD_BRIDGE_SECRETS-unset}"
"#,
        )
        .expect("write fake");
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        std::env::set_var(agent::API_KEY_ENV, "leaked");
        std::env::set_var(agent::SECRETS_ENV, "stdin");
        let completion =
            tokio::runtime::Runtime::new()
                .expect("runtime")
                .block_on(agent::run_claude_code(
                    "c",
                    &[],
                    "p",
                    repo.path(),
                    Some(bin.path().as_os_str().to_owned()),
                ));
        assert_eq!(
            completion.output,
            Some(serde_json::json!({ "text": "unset/unset" })),
            "{completion:?}"
        );
        agent::scrub_process_env();
        assert!(std::env::var_os(agent::API_KEY_ENV).is_none());
        assert!(std::env::var_os(agent::SECRETS_ENV).is_none());

        // Provider and other credential variables in the bridge's environment
        // (a library caller, or a host that passed them) never reach a tool
        // process the CLI starts: the CLI itself is started without them.
        let credentials = [
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "CODEX_API_KEY",
            "GITHUB_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
            agent::API_KEY_ENV,
        ];
        for name in credentials {
            std::env::set_var(name, "leaked-credential");
        }
        std::env::set_var("CODEX_HOME", "/codex-home");
        for (name, success) in [
            (
                "claude",
                r#"echo '{"type":"result","subtype":"success","is_error":false,"result":"ok"}'"#,
            ),
            (
                "codex",
                r#"echo '{"type":"item.completed","item":{"type":"agent_message","text":"ok"}}'"#,
            ),
        ] {
            let bin = tempfile::tempdir().expect("bin");
            let repo = tempfile::tempdir().expect("repo");
            std::fs::create_dir(repo.path().join(".git")).expect("git");
            let fake = bin.path().join(name);
            std::fs::write(
                &fake,
                format!(
                    r#"#!/bin/sh
here="$(dirname "$0")"
if [ "$1" = auth ] || [ "$1" = login ]; then echo '{{"loggedIn":true}}'; exit 0; fi
cat > /dev/null
# A "tool" the agent runs, as Claude Code's Bash or Codex's shell would.
sh -c 'env' > "$here/tool.env"
{success}
"#
                ),
            )
            .expect("write fake");
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).expect("chmod");
            let runtime = tokio::runtime::Runtime::new().expect("runtime");
            let path = Some(bin.path().as_os_str().to_owned());
            let completion = if name == "claude" {
                runtime.block_on(agent::run_claude_code("c", &[], "p", repo.path(), path))
            } else {
                runtime.block_on(agent::run_codex(
                    "c",
                    butterflow_execution_bridge::external::CodexSandbox::ReadOnly,
                    "p",
                    repo.path(),
                    path,
                ))
            };
            assert_eq!(
                completion.output,
                Some(serde_json::json!({ "text": "ok" })),
                "{name}: {completion:?}"
            );
            let tool_env = std::fs::read_to_string(bin.path().join("tool.env")).expect("tool env");
            for credential in credentials {
                assert!(
                    !tool_env.contains(&format!("{credential}=")),
                    "{name}: {credential} reached a tool process"
                );
            }
            assert!(!tool_env.contains("leaked-credential"), "{name}");
            // Non-secret CLI home overrides are kept.
            assert!(tool_env.contains("CODEX_HOME=/codex-home"), "{name}");
        }
        for name in credentials {
            std::env::remove_var(name);
        }
        std::env::remove_var("CODEX_HOME");
    }
}
