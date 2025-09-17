//! Status reporting tool for interactive mode
//! This tool allows the AI agent to communicate its current status to the user interface

use crate::interactive::message_handler::AppMessage;
use async_trait::async_trait;
use coro_core::tools::{Tool, ToolCall, ToolExample, ToolResult};
use serde_json::json;
use tokio::sync::broadcast;

/// Status reporting tool for interactive mode
/// This tool is designed to be used exclusively in interactive mode to provide
/// real-time status updates to the user interface
pub struct StatusReportTool {
    /// UI sender for broadcasting status updates
    ui_sender: Option<broadcast::Sender<AppMessage>>,
}

impl StatusReportTool {
    /// Create a new status report tool
    pub fn new() -> Self {
        Self { ui_sender: None }
    }

    /// Create a new status report tool with UI sender
    pub fn with_ui_sender(ui_sender: broadcast::Sender<AppMessage>) -> Self {
        Self {
            ui_sender: Some(ui_sender),
        }
    }

    /// Set the UI sender
    pub fn set_ui_sender(&mut self, ui_sender: broadcast::Sender<AppMessage>) {
        self.ui_sender = Some(ui_sender);
    }
}

#[async_trait]
impl Tool for StatusReportTool {
    fn name(&self) -> &str {
        "status_report"
    }

    fn description(&self) -> &str {
        "Report current status to the user interface in interactive mode. **IMPORTANT: You MUST use this tool every time you change what you're doing or start a new action.** This provides real-time feedback to users about your current activity (e.g., 'Analyzing code', 'Searching files', 'Reading documentation', 'Generating response'). Always update the status when transitioning between different types of work to keep users informed during long-running operations."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "status": {
                    "type": "string",
                    "description": "Current status message to display to the user. Use clear, action-oriented descriptions like 'Analyzing code', 'Searching files', 'Reading documentation', 'Generating response', 'Writing code', 'Running tests', etc. Update this every time you start a new type of activity. Limit the number of characters to 20"
                },
                "details": {
                    "type": "string",
                    "description": "Optional additional details about the current operation to provide more context to the user"
                }
            },
            "required": ["status"]
        })
    }

    async fn execute(&self, call: ToolCall) -> coro_core::error::Result<ToolResult> {
        let status: String = call.get_parameter("status")?;
        let details: Option<String> = call.get_parameter("details").ok();

        // Validate status message
        if status.trim().is_empty() {
            return Ok(ToolResult::error(
                call.id.clone(),
                "Status message cannot be empty".to_string(),
            ));
        }

        // Send status update to UI if sender is available
        if let Some(ui_sender) = &self.ui_sender {
            let _ = ui_sender.send(AppMessage::AgentTaskStarted {
                operation: status.clone(),
            });
        }

        // Create response message
        let mut response = format!("Status updated: {}", status);
        if let Some(details) = &details {
            response.push_str(&format!("\nDetails: {}", details));
        }

        Ok(ToolResult::success(&call.id, &response).with_data(json!({
            "status": status,
            "details": details,
            "timestamp": chrono::Utc::now().to_rfc3339()
        })))
    }

    fn requires_confirmation(&self) -> bool {
        false // Status reporting doesn't require confirmation
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![
            ToolExample {
                description: "Update status when starting to analyze code".to_string(),
                parameters: json!({
                    "status": "Analyzing code"
                }),
                expected_result: "Status updated: Analyzing code".to_string(),
            },
            ToolExample {
                description: "Update status when switching to file search with specific details".to_string(),
                parameters: json!({
                    "status": "Searching files",
                    "details": "Looking for configuration files in the project"
                }),
                expected_result: "Status updated: Searching files\nDetails: Looking for configuration files in the project".to_string(),
            },
            ToolExample {
                description: "Update status when transitioning to response generation".to_string(),
                parameters: json!({
                    "status": "Generating response"
                }),
                expected_result: "Status updated: Generating response".to_string(),
            },
            ToolExample {
                description: "Update status when beginning to read documentation".to_string(),
                parameters: json!({
                    "status": "Reading documentation",
                    "details": "Reviewing API documentation for the requested feature"
                }),
                expected_result: "Status updated: Reading documentation\nDetails: Reviewing API documentation for the requested feature".to_string(),
            },
            ToolExample {
                description: "Update status when starting to write or modify code".to_string(),
                parameters: json!({
                    "status": "Writing code",
                    "details": "Implementing the requested feature"
                }),
                expected_result: "Status updated: Writing code\nDetails: Implementing the requested feature".to_string(),
            },
        ]
    }
}

/// Factory for creating StatusReportTool instances
pub struct StatusReportToolFactory {
    ui_sender: Option<broadcast::Sender<AppMessage>>,
}

impl StatusReportToolFactory {
    pub fn new() -> Self {
        Self { ui_sender: None }
    }

    pub fn with_ui_sender(ui_sender: broadcast::Sender<AppMessage>) -> Self {
        Self {
            ui_sender: Some(ui_sender),
        }
    }
}

impl coro_core::tools::ToolFactory for StatusReportToolFactory {
    fn create(&self) -> Box<dyn Tool> {
        if let Some(ui_sender) = &self.ui_sender {
            Box::new(StatusReportTool::with_ui_sender(ui_sender.clone()))
        } else {
            Box::new(StatusReportTool::new())
        }
    }

    fn tool_name(&self) -> &str {
        "status_report"
    }

    fn tool_description(&self) -> &str {
        "Report current status to the user interface in interactive mode. MUST be used every time you change activities to keep users informed."
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coro_core::tools::ToolCall;
    use serde_json::json;

    #[tokio::test]
    async fn test_status_report_basic() {
        let tool = StatusReportTool::new();
        let call = ToolCall {
            id: "test_call".to_string(),
            name: "status_report".to_string(),
            parameters: json!({
                "status": "Testing status report"
            }),
            metadata: None,
        };

        let result = tool.execute(call).await.unwrap();
        assert!(result.success);
        assert!(result
            .content
            .contains("Status updated: Testing status report"));
    }

    #[tokio::test]
    async fn test_status_report_with_details() {
        let tool = StatusReportTool::new();
        let call = ToolCall {
            id: "test_call".to_string(),
            name: "status_report".to_string(),
            parameters: json!({
                "status": "Analyzing code",
                "details": "Reviewing function implementations"
            }),
            metadata: None,
        };

        let result = tool.execute(call).await.unwrap();
        assert!(result.success);
        assert!(result.content.contains("Status updated: Analyzing code"));
        assert!(result
            .content
            .contains("Details: Reviewing function implementations"));
    }

    #[tokio::test]
    async fn test_status_report_empty_status() {
        let tool = StatusReportTool::new();
        let call = ToolCall {
            id: "test_call".to_string(),
            name: "status_report".to_string(),
            parameters: json!({
                "status": ""
            }),
            metadata: None,
        };

        let result = tool.execute(call).await.unwrap();
        assert!(!result.success);
        assert!(result.content.contains("Status message cannot be empty"));
    }
}
