//! CLI tool registry with extended tools

use coro_core::tools::ToolRegistry;

/// Create a CLI-specific tool registry with all available tools
pub fn create_cli_tool_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::default(); // This gets core tools (thinking, task_done, mcp)

    // Register CLI-specific tools
    registry.register_factory(Box::new(crate::tools::BashToolFactory));
    registry.register_factory(Box::new(crate::tools::EditToolFactory));
    registry.register_factory(Box::new(crate::tools::GlobToolFactory));
    registry.register_factory(Box::new(crate::tools::JsonEditToolFactory));
    registry.register_factory(Box::new(crate::tools::CkgToolFactory));

    registry
}

/// Get the default CLI tool names (including the moved tools)
pub fn get_default_cli_tools() -> Vec<String> {
    vec![
        "bash".to_string(),
        "str_replace_based_edit_tool".to_string(),
        "glob".to_string(),
        "sequentialthinking".to_string(),
        "task_done".to_string(),
        "json_edit_tool".to_string(),
        "ckg_tool".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_registry_has_all_tools() {
        let registry = create_cli_tool_registry();
        let tools = registry.list_tools();

        // Expected tools including CLI-specific ones
        let expected_tools = vec![
            "bash",
            "str_replace_based_edit_tool",
            "glob",
            "sequentialthinking",
            "task_done",
            "json_edit_tool",
            "ckg_tool",
            "mcp_tool",
            // "status_report",
        ];

        println!("Available CLI tools: {:?}", tools);

        // Check that all expected tools are registered
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
        let registry = create_cli_tool_registry();

        // Test creating each CLI tool
        let tools_to_test = vec![
            "bash",
            "str_replace_based_edit_tool",
            "glob",
            "sequentialthinking",
            "task_done",
            "json_edit_tool",
            "ckg_tool",
            "mcp_tool",
            // "status_report",
        ];

        for tool_name in tools_to_test {
            let tool = registry.create_tool(tool_name);
            assert!(tool.is_some(), "Failed to create CLI tool '{}'", tool_name);

            let tool = tool.unwrap();
            assert_eq!(
                tool.name(),
                tool_name,
                "Tool name mismatch for '{}'",
                tool_name
            );
        }
    }

    #[test]
    fn test_default_cli_tools() {
        let default_tools = get_default_cli_tools();
        let registry = create_cli_tool_registry();

        // Verify all default tools can be created
        for tool_name in &default_tools {
            let tool = registry.create_tool(tool_name);
            assert!(
                tool.is_some(),
                "Default CLI tool '{}' cannot be created",
                tool_name
            );
        }
    }
}
