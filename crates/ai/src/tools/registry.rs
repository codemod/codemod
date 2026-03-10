//! CLI tool registry with native rig tools.

use rig::tool::server::{ToolServer, ToolServerError, ToolServerHandle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CliToolKind {
    Bash,
    Edit,
    Glob,
    SequentialThinking,
    TaskDone,
    JsonEdit,
    Ckg,
    Mcp,
}

#[derive(Debug, Clone, Copy)]
struct CliToolSpec {
    kind: CliToolKind,
    name: &'static str,
    default_enabled: bool,
}

const CLI_TOOL_SPECS: &[CliToolSpec] = &[
    CliToolSpec {
        kind: CliToolKind::Bash,
        name: "bash",
        default_enabled: true,
    },
    CliToolSpec {
        kind: CliToolKind::Edit,
        name: "str_replace_based_edit_tool",
        default_enabled: true,
    },
    CliToolSpec {
        kind: CliToolKind::Glob,
        name: "glob",
        default_enabled: true,
    },
    CliToolSpec {
        kind: CliToolKind::SequentialThinking,
        name: "sequentialthinking",
        default_enabled: true,
    },
    CliToolSpec {
        kind: CliToolKind::TaskDone,
        name: "task_done",
        default_enabled: true,
    },
    CliToolSpec {
        kind: CliToolKind::JsonEdit,
        name: "json_edit_tool",
        default_enabled: true,
    },
    CliToolSpec {
        kind: CliToolKind::Ckg,
        name: "ckg_tool",
        default_enabled: true,
    },
    CliToolSpec {
        kind: CliToolKind::Mcp,
        name: "mcp_tool",
        default_enabled: false,
    },
];

fn find_tool_spec(name: &str) -> Option<CliToolSpec> {
    CLI_TOOL_SPECS
        .iter()
        .copied()
        .find(|tool| tool.name == name)
}

/// Lightweight registry for creating CLI tools by name.
#[derive(Debug, Clone, Copy, Default)]
pub struct CliToolRegistry;

impl CliToolRegistry {
    async fn add_tool_kind_to_server(
        &self,
        kind: CliToolKind,
        server: &ToolServerHandle,
    ) -> Result<(), ToolServerError> {
        match kind {
            CliToolKind::Bash => server.add_tool(crate::tools::bash::BashTool::new()).await?,
            CliToolKind::Edit => server.add_tool(crate::tools::edit::EditTool::new()).await?,
            CliToolKind::Glob => server.add_tool(crate::tools::glob::GlobTool::new()).await?,
            CliToolKind::SequentialThinking => {
                server
                    .add_tool(crate::tools::sequential_thinking::ThinkingTool::new())
                    .await?
            }
            CliToolKind::TaskDone => {
                server
                    .add_tool(crate::tools::task_done::TaskDoneTool::new())
                    .await?
            }
            CliToolKind::JsonEdit => {
                server
                    .add_tool(crate::tools::json_edit::JsonEditTool::new())
                    .await?
            }
            CliToolKind::Ckg => server.add_tool(crate::tools::ckg::CkgTool::new()).await?,
            CliToolKind::Mcp => server.add_tool(crate::tools::mcp::McpTool::new()).await?,
        }

        Ok(())
    }

    pub async fn add_tool_to_server(
        &self,
        name: &str,
        server: &ToolServerHandle,
    ) -> Result<bool, ToolServerError> {
        match find_tool_spec(name) {
            Some(spec) => {
                self.add_tool_kind_to_server(spec.kind, server).await?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    #[cfg(test)]
    pub fn list_tools() -> Vec<&'static str> {
        CLI_TOOL_SPECS.iter().map(|tool| tool.name).collect()
    }
}

/// Create a CLI-specific tool registry with all available tools.
pub fn create_cli_tool_registry() -> CliToolRegistry {
    CliToolRegistry
}

/// Build a tool server handle populated with the requested CLI tools.
///
/// Unknown tool names are ignored to preserve previous behavior.
pub async fn create_cli_tool_server_handle(
    tool_names: &[String],
) -> Result<ToolServerHandle, ToolServerError> {
    let registry = create_cli_tool_registry();
    let handle = ToolServer::new().run();

    for tool_name in tool_names {
        let _ = registry.add_tool_to_server(tool_name, &handle).await?;
    }

    Ok(handle)
}

/// Get the default CLI tool names (including moved tools).
pub fn get_default_cli_tools() -> Vec<String> {
    CLI_TOOL_SPECS
        .iter()
        .filter(|tool| tool.default_enabled)
        .map(|tool| tool.name.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn test_cli_registry_has_all_tools() {
        let tools = CliToolRegistry::list_tools();

        let expected_tools = vec![
            "bash",
            "str_replace_based_edit_tool",
            "glob",
            "sequentialthinking",
            "task_done",
            "json_edit_tool",
            "ckg_tool",
            "mcp_tool",
        ];

        for expected_tool in &expected_tools {
            assert!(
                tools.contains(expected_tool),
                "Tool '{}' is not registered in the CLI registry",
                expected_tool
            );
        }
    }

    #[test]
    fn test_cli_tool_creation() {
        let tools = CliToolRegistry::list_tools();
        assert!(!tools.is_empty());
    }

    #[test]
    fn test_default_cli_tools() {
        let default_tools = get_default_cli_tools();
        let tools = CliToolRegistry::list_tools();

        for tool_name in &default_tools {
            assert!(
                tools.contains(&tool_name.as_str()),
                "Default CLI tool '{}' is not listed",
                tool_name
            );
        }
    }

    #[test]
    fn test_default_cli_tools_contract_order() {
        let expected = vec![
            "bash".to_string(),
            "str_replace_based_edit_tool".to_string(),
            "glob".to_string(),
            "sequentialthinking".to_string(),
            "task_done".to_string(),
            "json_edit_tool".to_string(),
            "ckg_tool".to_string(),
        ];

        assert_eq!(get_default_cli_tools(), expected);
    }

    #[tokio::test]
    async fn test_cli_registry_glob_execution_contract() {
        let temp_dir = TempDir::new().unwrap();
        let expected_file = temp_dir.path().join("file.txt");
        std::fs::write(&expected_file, "hello").unwrap();

        let handle = create_cli_tool_server_handle(&["glob".to_string()])
            .await
            .unwrap();

        let args = json!({
            "pattern": "*.txt",
            "base_path": temp_dir.path().to_string_lossy().to_string(),
            "files_only": true
        });

        let result = handle.call_tool("glob", &args.to_string()).await.unwrap();
        assert!(
            result.contains("Found 1 files matching pattern"),
            "glob content should report one match: {}",
            result
        );
    }

    #[tokio::test]
    async fn test_create_cli_tool_server_handle_registers_tools() {
        let requested = vec![
            "glob".to_string(),
            "task_done".to_string(),
            "unknown_tool".to_string(),
        ];

        let handle = create_cli_tool_server_handle(&requested).await.unwrap();
        let definitions = handle.get_tool_defs(None).await.unwrap();
        let names: Vec<String> = definitions.into_iter().map(|d| d.name).collect();

        assert!(names.contains(&"glob".to_string()));
        assert!(names.contains(&"task_done".to_string()));
        assert!(!names.contains(&"unknown_tool".to_string()));
    }

    #[tokio::test]
    async fn test_add_tool_to_server_unknown_name_is_ignored() {
        let registry = create_cli_tool_registry();
        let handle = ToolServer::new().run();
        let added = registry
            .add_tool_to_server("unknown_tool", &handle)
            .await
            .unwrap();
        assert!(!added);

        let definitions = handle.get_tool_defs(None).await.unwrap();
        assert!(definitions.is_empty());
    }

    #[tokio::test]
    async fn test_tool_creation_and_server_registration_name_parity() {
        let registry = create_cli_tool_registry();
        let handle = ToolServer::new().run();

        for tool_name in CliToolRegistry::list_tools() {
            let added = registry
                .add_tool_to_server(tool_name, &handle)
                .await
                .unwrap();
            assert!(added, "tool should register into server: {}", tool_name);
        }

        let definitions = handle.get_tool_defs(None).await.unwrap();
        let names: Vec<String> = definitions.into_iter().map(|d| d.name).collect();
        for tool_name in CliToolRegistry::list_tools() {
            assert!(names.contains(&tool_name.to_string()));
        }
    }
}
