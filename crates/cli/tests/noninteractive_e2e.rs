//! The stable command surface must work with CI=1 and stdin closed.

use std::convert::Infallible;
use std::fs;
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Response, Server, StatusCode};
use serde_json::{json, Value};

fn write(root: &Path, path: &str, contents: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn run(root: &Path, registry: &str, args: &[&str], code: i32) -> Output {
    // Capture in files so even excessive output can't block the subprocess
    // on a full pipe before the deadline.
    let stdout = root.join("stdout");
    let stderr = root.join("stderr");
    let mut child = Command::new(env!("CARGO_BIN_EXE_codemod"))
        .args(args)
        .current_dir(root)
        .env("CI", "1")
        .env("NO_COLOR", "1")
        .env("DO_NOT_TRACK", "1")
        .env("CODEMOD_REGISTRY_URL", registry)
        .env("CODEMOD_RUN_STORE_DIR", root.join("runs"))
        .env("HOME", root.join("home"))
        .env("USERPROFILE", root.join("home"))
        .env("XDG_CONFIG_HOME", root.join("home"))
        .env("APPDATA", root.join("home"))
        .env("NO_PROXY", "127.0.0.1")
        .env_remove("CODEMOD_AUTH_TOKEN")
        .env_remove("CODEMOD_V1")
        .env_remove("RUST_LOG")
        .stdin(Stdio::null())
        .stdout(fs::File::create(&stdout).unwrap())
        .stderr(fs::File::create(&stderr).unwrap())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "{args:?} hung:\n{}\n{}",
                fs::read_to_string(stdout).unwrap(),
                fs::read_to_string(stderr).unwrap()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let output = Output {
        status,
        stdout: fs::read(stdout).unwrap(),
        stderr: fs::read(stderr).unwrap(),
    };
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.status.code(), Some(code), "{args:?}:\n{text}");
    for marker in [
        "not a TTY",
        "Failed to get user input",
        "Please open the following URL",
        "Codemod collects CLI usage",
        "? ",
        "(y/N)",
    ] {
        assert!(
            !text
                .lines()
                .any(|line| line.starts_with(marker) || (marker != "? " && line.contains(marker))),
            "{args:?} prompted:\n{text}"
        );
    }
    output
}

fn document(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&output.stdout)))
}

#[tokio::test]
async fn stable_commands_do_not_prompt_or_hang_in_ci() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let registry = format!("http://{}", listener.local_addr().unwrap());
    let registry_server = Server::from_tcp(listener)
        .unwrap()
        .serve(make_service_fn(|_| async {
            Ok::<_, Infallible>(service_fn(|request| async move {
                let method = request.method().as_str();
                let path = request.uri().path();
                let authorized = request
                    .headers()
                    .get("authorization")
                    .is_some_and(|header| header == "Bearer fixture-secret");
                let body = match (method, path, authorized) {
                    ("GET", "/api/v1/registry/search", _) => json!({"total": 0, "packages": []}),
                    ("GET", "/api/auth/oauth2/userinfo", true) => json!({
                        "id": "fixture-user",
                        "username": "fixture",
                        "email": "fixture@example.invalid",
                    }),
                    ("POST", "/api/v1/registry/packages/ci-fixture", true) => {
                        assert!(!hyper::body::to_bytes(request.into_body())
                            .await
                            .unwrap()
                            .is_empty());
                        json!({
                            "success": true,
                            "package": {
                                "id": "fixture-package",
                                "name": "ci-fixture",
                                "version": "1.0.0",
                                "scope": null,
                                "published_at": "2026-01-01T00:00:00Z",
                            }
                        })
                    }
                    _ => {
                        return Ok::<_, Infallible>(
                            Response::builder()
                                .status(StatusCode::NOT_FOUND)
                                .body(Body::empty())
                                .unwrap(),
                        )
                    }
                };
                Ok(Response::new(Body::from(body.to_string())))
            }))
        }));
    let server = tokio::spawn(registry_server);
    tokio::task::spawn_blocking(move || {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("home")).unwrap();
        fs::create_dir(root.join("target")).unwrap();
        let cli = |args: &[&str], code| run(root, &registry, args, code);

        write(root, "package/codemod.yaml", "schema_version: \"1.0\"\nname: ci-fixture\nversion: 1.0.0\ndescription: Fixture\nauthor: Test\nworkflow: workflow.ts\n");
        write(root, "package/workflow.ts", r#"
import { fail } from "codemod/workflow";
export default async function main() { fail("intentional CI result"); }
"#);
        let local_package = root.join("package");
        let partial = cli(&["run", local_package.to_str().unwrap(), "--target", "target"], 2);
        assert!(String::from_utf8_lossy(&partial.stderr).contains("intentional CI result"));

        write(root, "package/workflow.ts", r#"
import { step, approval, forEachFile } from "codemod/workflow";
export default async function main() {
  await step(approval("Continue the fixture?"));
  await step("Check", forEachFile("*.ts", (file) => file.text));
  return "finished";
}
"#);
        cli(&["workflow", "validate", "-w", "package"], 0);
        let start = || {
            let result = cli(&["workflow", "run", "-w", "package", "--target", "target"], 3);
            assert!(String::from_utf8_lossy(&result.stderr).contains("Waiting for approval:"));
            let runs = document(&cli(&["workflow", "list", "--format", "json"], 0));
            runs.as_array().unwrap().iter()
                .find(|run| run["status"] == "awaiting-approval")
                .unwrap()["id"].as_str().unwrap().to_owned()
        };
        let status = |id| document(&cli(&["workflow", "status", "--id", id, "--format", "json"], 0));
        let id = start();
        let waiting = status(&id);
        assert_eq!(waiting["status"], "awaiting-approval");
        let question = waiting["question"].as_str().unwrap();
        cli(&["workflow", "resume", "--id", &id, "--approve", question], 0);
        assert_eq!(status(&id)["output"], "finished");
        let cancel = start();
        cli(&["workflow", "cancel", "--id", &cancel], 0);
        assert_eq!(status(&cancel)["status"], "canceled");

        write(root, "legacy/workflow.yaml", "version: \"1\"\nnodes:\n  - id: run\n    name: Run\n    steps:\n      - name: Hello\n        run: echo hello\n");
        cli(&["workflow", "upgrade", "legacy"], 0);
        assert!(root.join("legacy/workflow.ts").is_file());

        let before = "const value = 'OLD';\n";
        let after = "const value = 'NEW';\n";
        write(root, "transform.ts", "export default function transform(root: any) { return root.root().text().replace('OLD', 'NEW'); }");
        write(root, "target/main.ts", before);
        cli(&["jssg", "run", "transform.ts", "--language", "typescript", "--target", "target", "--allow-dirty"], 0);
        assert_eq!(fs::read_to_string(root.join("target/main.ts")).unwrap(), after);
        write(root, "fixtures/rewrite/input/main.ts", before);
        write(root, "fixtures/rewrite/expected/main.ts", after);
        cli(&["jssg", "test", "transform.ts", "fixtures", "--language", "typescript"], 0);

        let login_needed = cli(&["login", "--format", "json"], 1);
        assert_eq!(document(&login_needed)["logged_in"], false);
        assert!(String::from_utf8_lossy(&login_needed.stderr).contains("--api-key"));
        let login = document(&cli(&["login", "--api-key", "fixture-secret", "--format", "json"], 0));
        assert_eq!(login["logged_in"], true);
        assert_eq!(login["user"]["username"], "fixture");
        assert_eq!(document(&cli(&["login", "--format", "json"], 0)), login);
        let who = document(&cli(&["whoami", "--detailed", "--format", "json"], 0));
        assert_eq!(who["user"], login["user"]);
        assert_eq!(who["token"]["type"], "Bearer");
        for result in [&login, &who] {
            assert_eq!(result["registry"], registry);
            assert!(!result.to_string().contains("fixture-secret"));
        }
        let publish = document(&cli(&["publish", "package", "--tag", "preview", "--format", "json"], 0));
        assert_eq!(publish["success"], true);
        assert_eq!(publish["package"]["name"], "ci-fixture");
        assert_eq!(publish["tag"], "preview");
        assert_eq!(document(&cli(&["search", "fixture", "--format", "json"], 0))["total"], 0);
        cli(&["logout"], 0);
        let logged_out = document(&cli(&["whoami", "--format", "json"], 1));
        assert_eq!(logged_out["logged_in"], false);
        assert!(logged_out["user"].is_null());

        cli(&["init", "new-package", "--no-interactive", "--project-type", "shell",
              "--package-manager", "npm", "--language", "typescript", "--description", "Fixture",
              "--author", "Test", "--license", "MIT"], 0);
        assert!(root.join("new-package/codemod.yaml").is_file());
    }).await.unwrap();
    server.abort();
}
