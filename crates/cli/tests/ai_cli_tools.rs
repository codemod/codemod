use std::io::Write;
use std::process::{Command, Stdio};

fn codemod_binary() -> &'static str {
    env!("CARGO_BIN_EXE_codemod")
}

fn codemod_command() -> Command {
    let mut command = Command::new(codemod_binary());
    command.arg("--disable-analytics");
    command
}

fn codemod_command_offline_docs() -> Command {
    let mut command = codemod_command();
    command.env("CODEMOD_MCP_PUBLIC_DOCS_OFFLINE", "1");
    command
}

#[test]
fn ai_dump_ast_reads_stdin_with_option_terminator() {
    let mut child = codemod_command()
        .args(["ai", "dump-ast", "--"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("failed to spawn codemod ai dump-ast");

    child
        .stdin
        .as_mut()
        .expect("expected stdin")
        .write_all(b"const x = 23;")
        .expect("failed to write stdin");

    let output = child
        .wait_with_output()
        .expect("failed to wait for codemod ai dump-ast");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("program"));
    assert!(stdout.contains("lexical_declaration"));
}

#[test]
fn ai_node_types_prints_language_node_types() {
    let output = codemod_command()
        .args(["ai", "node-types", "tsx"])
        .output()
        .expect("failed to run codemod ai node-types");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("<TREE_SITTER_NODE_TYPES>"));
    assert!(stdout.contains("jsx_element"));
}

#[test]
fn ai_docs_without_query_lists_resources() {
    let output = codemod_command()
        .args(["ai", "docs"])
        .output()
        .expect("failed to run codemod ai docs");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("jssg-instructions"));
    assert!(stdout.contains("codemod-cli://instructions"));
}

#[test]
fn ai_tools_json_lists_mcp_equivalent_tools() {
    let output = codemod_command()
        .args(["ai", "tools", "--json"])
        .output()
        .expect("failed to run codemod ai tools");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"dump_ast\""));
    assert!(stdout.contains("\"get_node_types\""));
    assert!(stdout.contains("\"validate_codemod_package\""));
}

#[test]
fn ai_tool_accepts_node_types_alias() {
    let output = codemod_command()
        .args(["ai", "tool", "node-types"])
        .output()
        .expect("failed to run codemod ai tool");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("get_node_types"));
    assert!(stdout.contains("\"language\""));
}

#[test]
fn ai_tool_unknown_name_fails() {
    let output = codemod_command()
        .args(["ai", "tool", "missing-tool"])
        .output()
        .expect("failed to run codemod ai tool");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Unknown MCP tool 'missing-tool'"));
}

#[test]
fn ai_call_invokes_node_types_alias() {
    let output = codemod_command()
        .args([
            "ai",
            "call",
            "node-types",
            "--input",
            r#"{"language":"tsx"}"#,
        ])
        .output()
        .expect("failed to run codemod ai call");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("<TREE_SITTER_NODE_TYPES>"));
    assert!(stdout.contains("jsx_element"));
}

#[test]
fn ai_call_invalid_json_fails() {
    let output = codemod_command()
        .args(["ai", "call", "node-types", "--input", "{"])
        .output()
        .expect("failed to run codemod ai call");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Failed to parse JSON input"));
}

#[test]
fn ai_resources_lists_resources() {
    let output = codemod_command()
        .args(["ai", "resources"])
        .output()
        .expect("failed to run codemod ai resources");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("codemod-cli-instructions"));
    assert!(stdout.contains("jssg-utils://instructions"));
}

#[test]
fn ai_resource_reads_resource_by_name() {
    let output = codemod_command_offline_docs()
        .args(["ai", "resource", "codemod-cli", "--json"])
        .output()
        .expect("failed to run codemod ai resource");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"name\": \"codemod-cli-instructions\""));
    assert!(stdout.contains("These instructions are bundled from this release"));
}

#[test]
fn ai_resource_unknown_name_fails() {
    let output = codemod_command()
        .args(["ai", "resource", "missing-resource"])
        .output()
        .expect("failed to run codemod ai resource");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Unknown MCP resource 'missing-resource'"));
}

#[test]
fn ai_docs_reads_resource_query() {
    let output = codemod_command_offline_docs()
        .args(["ai", "docs", "codemod-cli"])
        .output()
        .expect("failed to run codemod ai docs");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Canonical Codemod CLI and Workflow Documentation"));
    assert!(stdout.contains("<!-- Local source: docs/cli.mdx -->"));
}

#[test]
fn ai_docs_search_json_returns_matches() {
    let output = codemod_command_offline_docs()
        .args(["ai", "docs", "CLI Command Reference", "--json"])
        .output()
        .expect("failed to run codemod ai docs search");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("\"query\": \"CLI Command Reference\""));
    assert!(stdout.contains("\"matches\""));
    assert!(stdout.contains("codemod-cli-instructions"));
}
