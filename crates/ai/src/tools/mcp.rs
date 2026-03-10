//! MCP (Model Context Protocol) tool support

use crate::tools::core::Result;
use crate::tools::core::{ToolCall, ToolResult};
use rmcp::model::{CallToolRequestParam, Tool};
use rmcp::service::{RoleClient, RunningService, ServiceExt};
use rmcp::transport::TokioChildProcess;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::Mutex;
use tokio::time::{timeout, Duration};

/// MCP server configuration
#[derive(Debug, Clone)]
pub struct McpServerConfig {
    pub command: Vec<String>,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub timeout_seconds: u64,
}

type McpClientService = RunningService<RoleClient, ()>;

/// MCP server instance
pub struct McpServer {
    config: McpServerConfig,
    service: Option<McpClientService>,
    started: bool,
}

impl McpServer {
    pub fn new(config: McpServerConfig) -> Self {
        Self {
            config,
            service: None,
            started: false,
        }
    }

    /// Start the MCP server process
    pub async fn start(&mut self) -> Result<()> {
        if self.started {
            return Ok(());
        }

        let mut cmd = Command::new(&self.config.command[0]);
        if self.config.command.len() > 1 {
            cmd.args(&self.config.command[1..]);
        }
        cmd.args(&self.config.args);

        for (key, value) in &self.config.env {
            cmd.env(key, value);
        }

        let transport = TokioChildProcess::new(cmd)?;
        let service = timeout(
            Duration::from_secs(self.config.timeout_seconds),
            ().serve(transport),
        )
        .await
        .map_err(|_| {
            format!(
                "MCP server initialization timed out after {} seconds",
                self.config.timeout_seconds
            )
        })?
        .map_err(|e| e.to_string())?;

        self.service = Some(service);
        self.started = true;

        Ok(())
    }

    /// Stop the MCP server
    pub async fn stop(&mut self) {
        if let Some(service) = self.service.take() {
            let _ = service.cancel().await;
        }
        self.started = false;
    }

    fn peer(&self) -> Result<rmcp::service::ServerSink> {
        let service = self
            .service
            .as_ref()
            .ok_or_else(|| crate::tools::core::Error::new("MCP server not started"))?;
        Ok(service.peer().clone())
    }

    /// List available tools from MCP server
    pub async fn list_tools(&self) -> Result<Vec<Value>> {
        let peer = self.peer()?;

        let tools = timeout(
            Duration::from_secs(self.config.timeout_seconds),
            peer.list_all_tools(),
        )
        .await
        .map_err(|_| {
            format!(
                "Timed out listing MCP tools after {} seconds",
                self.config.timeout_seconds
            )
        })?
        .map_err(|e| e.to_string())?;

        Ok(tools.into_iter().map(tool_to_json).collect())
    }

    /// Call a tool on the MCP server
    pub async fn call_tool(&self, tool_name: &str, arguments: Value) -> Result<Value> {
        let peer = self.peer()?;

        let object_arguments = arguments.as_object().cloned().ok_or_else(|| {
            crate::tools::core::Error::new("tool_arguments must be a JSON object")
        })?;

        let result = timeout(
            Duration::from_secs(self.config.timeout_seconds),
            peer.call_tool(CallToolRequestParam {
                name: tool_name.to_string().into(),
                arguments: Some(object_arguments),
                task: None,
            }),
        )
        .await
        .map_err(|_| {
            format!(
                "Timed out calling MCP tool '{}' after {} seconds",
                tool_name, self.config.timeout_seconds
            )
        })?
        .map_err(|e| e.to_string())?;

        serde_json::to_value(result).map_err(Into::into)
    }
}

fn tool_to_json(tool: Tool) -> Value {
    let fallback_name = tool.name.to_string();
    let fallback_description = tool.description.as_deref().unwrap_or("").to_string();
    let fallback_input_schema = tool.schema_as_json_value();

    serde_json::to_value(tool).unwrap_or_else(|_| {
        json!({
            "name": fallback_name,
            "description": fallback_description,
            "inputSchema": fallback_input_schema,
        })
    })
}

/// Tool for interacting with MCP servers
pub struct McpTool {
    servers: Arc<Mutex<HashMap<String, McpServer>>>,
}

impl Default for McpTool {
    fn default() -> Self {
        Self::new()
    }
}

impl McpTool {
    pub fn new() -> Self {
        Self {
            servers: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl McpTool {
    fn name(&self) -> &str {
        "mcp_tool"
    }

    fn description(&self) -> &str {
        "Tool for interacting with MCP (Model Context Protocol) servers\n\
         * Manages connections to external MCP servers\n\
         * Provides access to tools exposed by MCP servers\n\
         * Supports server lifecycle management (start, stop, restart)\n\
         * Handles JSON-RPC communication with MCP servers\n\
         \n\
         Operations:\n\
         - `start_server`: Start an MCP server with given configuration\n\
         - `stop_server`: Stop a running MCP server\n\
         - `list_servers`: List all configured MCP servers\n\
         - `list_tools`: List tools available from a specific MCP server\n\
         - `call_tool`: Call a tool on a specific MCP server\n\
         \n\
         MCP servers are external processes that expose tools and resources\n\
         through the Model Context Protocol. This allows integration with\n\
         various external systems and services."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["start_server", "stop_server", "list_servers", "list_tools", "call_tool"],
                    "description": "The operation to perform"
                },
                "server_name": {
                    "type": "string",
                    "description": "Name of the MCP server (required for most operations)"
                },
                "command": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Command to start the MCP server (required for start_server)"
                },
                "args": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Arguments for the MCP server command"
                },
                "env": {
                    "type": "object",
                    "description": "Environment variables for the MCP server"
                },
                "timeout_seconds": {
                    "type": "integer",
                    "description": "Timeout for MCP server operations in seconds (default: 30)"
                },
                "tool_name": {
                    "type": "string",
                    "description": "Name of the tool to call (required for call_tool)"
                },
                "tool_arguments": {
                    "type": "object",
                    "description": "Arguments to pass to the tool (required for call_tool)"
                }
            },
            "required": ["operation"]
        })
    }

    async fn execute(&self, call: ToolCall) -> Result<ToolResult> {
        let operation: String = call.get_parameter("operation")?;

        match operation.as_str() {
            "start_server" => {
                let server_name: String = call.get_parameter("server_name")?;
                let command: Vec<String> = call.get_parameter("command")?;
                let args: Vec<String> = call.get_parameter_or("args", Vec::new());
                let env: HashMap<String, String> = call.get_parameter_or("env", HashMap::new());
                let timeout_seconds: u64 = call.get_parameter_or("timeout_seconds", 30);
                self.start_server(&call.id, server_name, command, args, env, timeout_seconds)
                    .await
            }
            "stop_server" => {
                let server_name: String = call.get_parameter("server_name")?;
                self.stop_server(&call.id, server_name).await
            }
            "list_servers" => self.list_servers(&call.id).await,
            "list_tools" => {
                let server_name: String = call.get_parameter("server_name")?;
                self.list_tools(&call.id, server_name).await
            }
            "call_tool" => {
                let server_name: String = call.get_parameter("server_name")?;
                let tool_name: String = call.get_parameter("tool_name")?;
                let tool_arguments: Value = call.get_parameter("tool_arguments")?;
                self.call_tool(&call.id, server_name, tool_name, tool_arguments)
                    .await
            }
            _ => Ok(ToolResult::error(
                &call.id,
                format!(
                    "Unknown operation: {}. Supported operations: start_server, stop_server, list_servers, list_tools, call_tool",
                    operation
                ),
            )),
        }
    }
}

impl McpTool {
    /// Start an MCP server
    async fn start_server(
        &self,
        call_id: &str,
        server_name: String,
        command: Vec<String>,
        args: Vec<String>,
        env: HashMap<String, String>,
        timeout_seconds: u64,
    ) -> Result<ToolResult> {
        if command.is_empty() {
            return Ok(ToolResult::error(call_id, "Command cannot be empty"));
        }

        let config = McpServerConfig {
            command,
            args,
            env,
            timeout_seconds,
        };

        let mut server = McpServer::new(config);

        match server.start().await {
            Ok(()) => {
                let mut servers = self.servers.lock().await;
                servers.insert(server_name.clone(), server);

                Ok(ToolResult::success(
                    call_id,
                    format!("MCP server '{}' started successfully", server_name),
                ))
            }
            Err(e) => Ok(ToolResult::error(
                call_id,
                format!("Failed to start MCP server '{}': {}", server_name, e),
            )),
        }
    }

    /// Stop an MCP server
    async fn stop_server(&self, call_id: &str, server_name: String) -> Result<ToolResult> {
        let mut server = {
            let mut servers = self.servers.lock().await;
            servers.remove(&server_name)
        };

        if let Some(ref mut server) = server {
            server.stop().await;
            Ok(ToolResult::success(
                call_id,
                format!("MCP server '{}' stopped successfully", server_name),
            ))
        } else {
            Ok(ToolResult::error(
                call_id,
                format!("MCP server '{}' not found", server_name),
            ))
        }
    }

    /// List all MCP servers
    async fn list_servers(&self, call_id: &str) -> Result<ToolResult> {
        let servers = self.servers.lock().await;

        if servers.is_empty() {
            return Ok(ToolResult::success(
                call_id,
                "No MCP servers are currently running",
            ));
        }

        let mut result = String::from("Running MCP servers:\n\n");
        for (name, server) in servers.iter() {
            result.push_str(&format!(
                "- {} (command: {:?}, started: {})\n",
                name, server.config.command, server.started
            ));
        }

        Ok(ToolResult::success(call_id, &result))
    }

    /// List tools from an MCP server
    async fn list_tools(&self, call_id: &str, server_name: String) -> Result<ToolResult> {
        let servers = self.servers.lock().await;

        if let Some(server) = servers.get(&server_name) {
            match server.list_tools().await {
                Ok(tools) => {
                    if tools.is_empty() {
                        Ok(ToolResult::success(
                            call_id,
                            format!("No tools available from MCP server '{}'", server_name),
                        ))
                    } else {
                        let mut result =
                            format!("Tools available from MCP server '{}':\n\n", server_name);

                        for (i, tool) in tools.iter().enumerate() {
                            if let Some(name) = tool.get("name").and_then(|n| n.as_str()) {
                                result.push_str(&format!("{}. {}", i + 1, name));

                                if let Some(description) =
                                    tool.get("description").and_then(|d| d.as_str())
                                {
                                    result.push_str(&format!(" - {}", description));
                                }
                                result.push('\n');

                                if let Some(input_schema) = tool.get("inputSchema") {
                                    result.push_str(&format!(
                                        "   Input schema: {}\n",
                                        serde_json::to_string_pretty(input_schema)
                                            .unwrap_or_default()
                                    ));
                                }
                                result.push('\n');
                            }
                        }

                        Ok(ToolResult::success(call_id, &result))
                    }
                }
                Err(e) => Ok(ToolResult::error(
                    call_id,
                    format!(
                        "Failed to list tools from MCP server '{}': {}",
                        server_name, e
                    ),
                )),
            }
        } else {
            Ok(ToolResult::error(
                call_id,
                format!("MCP server '{}' not found", server_name),
            ))
        }
    }

    /// Call a tool on an MCP server
    async fn call_tool(
        &self,
        call_id: &str,
        server_name: String,
        tool_name: String,
        tool_arguments: Value,
    ) -> Result<ToolResult> {
        let servers = self.servers.lock().await;

        if let Some(server) = servers.get(&server_name) {
            match server.call_tool(&tool_name, tool_arguments).await {
                Ok(result) => {
                    let result_str = if result.is_string() {
                        result.as_str().unwrap_or("").to_string()
                    } else {
                        serde_json::to_string_pretty(&result).unwrap_or_default()
                    };

                    Ok(ToolResult::success(
                        call_id,
                        format!(
                            "Tool '{}' executed successfully on MCP server '{}':\n\n{}",
                            tool_name, server_name, result_str
                        ),
                    ))
                }
                Err(e) => Ok(ToolResult::error(
                    call_id,
                    format!(
                        "Failed to call tool '{}' on MCP server '{}': {}",
                        tool_name, server_name, e
                    ),
                )),
            }
        } else {
            Ok(ToolResult::error(
                call_id,
                format!("MCP server '{}' not found", server_name),
            ))
        }
    }
}

crate::impl_rig_tooldyn!(McpTool);

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_list_servers_when_empty() {
        let tool = McpTool::new();
        let call = ToolCall::new("mcp_tool", json!({"operation": "list_servers"}));

        let result = tool.execute(call).await.unwrap();
        assert!(result.success);
        assert!(result
            .content
            .contains("No MCP servers are currently running"));
    }

    #[tokio::test]
    async fn test_start_server_rejects_empty_command() {
        let tool = McpTool::new();
        let call = ToolCall::new(
            "mcp_tool",
            json!({
                "operation": "start_server",
                "server_name": "test",
                "command": []
            }),
        );

        let result = tool.execute(call).await.unwrap();
        assert!(!result.success);
        assert!(result.content.contains("Command cannot be empty"));
    }

    #[tokio::test]
    async fn test_stop_server_not_found() {
        let tool = McpTool::new();
        let call = ToolCall::new(
            "mcp_tool",
            json!({
                "operation": "stop_server",
                "server_name": "missing"
            }),
        );

        let result = tool.execute(call).await.unwrap();
        assert!(!result.success);
        assert!(result.content.contains("MCP server 'missing' not found"));
    }

    #[tokio::test]
    async fn test_list_tools_server_not_found() {
        let tool = McpTool::new();
        let call = ToolCall::new(
            "mcp_tool",
            json!({
                "operation": "list_tools",
                "server_name": "missing"
            }),
        );

        let result = tool.execute(call).await.unwrap();
        assert!(!result.success);
        assert!(result.content.contains("MCP server 'missing' not found"));
    }

    #[tokio::test]
    async fn test_call_tool_server_not_found() {
        let tool = McpTool::new();
        let call = ToolCall::new(
            "mcp_tool",
            json!({
                "operation": "call_tool",
                "server_name": "missing",
                "tool_name": "anything",
                "tool_arguments": {}
            }),
        );

        let result = tool.execute(call).await.unwrap();
        assert!(!result.success);
        assert!(result.content.contains("MCP server 'missing' not found"));
    }
}
