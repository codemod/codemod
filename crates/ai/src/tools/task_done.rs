//! Task completion tool

use crate::tools::core::{ToolCall, ToolResult};
use serde_json::json;

fn render_task_done_result(summary: &str, details: Option<&str>) -> String {
    let mut result = format!("Summary: {summary}");

    if let Some(details) = details {
        result.push_str("\n\nDetails:\n");
        result.push_str(details);
    }

    result
}

/// Tool for marking tasks as completed
pub struct TaskDoneTool;

impl TaskDoneTool {
    pub fn new() -> Self {
        Self
    }
}

impl TaskDoneTool {
    fn name(&self) -> &str {
        "task_done"
    }

    fn description(&self) -> &str {
        "Mark a task as completed. Use this when you have successfully \
         completed the requested task and want to signal completion."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "Summary of what was accomplished"
                },
                "details": {
                    "type": "string",
                    "description": "Optional detailed description of the work done"
                }
            },
            "required": ["summary"]
        })
    }

    async fn execute(&self, call: ToolCall) -> crate::tools::core::Result<ToolResult> {
        let summary: String = call.get_parameter("summary")?;
        let details: Option<String> = call.get_parameter("details").ok();
        let result = render_task_done_result(&summary, details.as_deref());

        Ok(ToolResult::success(&call.id, &result).with_data(json!({
            "task_completed": true,
            "summary": summary,
            "details": details
        })))
    }
}

impl Default for TaskDoneTool {
    fn default() -> Self {
        Self::new()
    }
}

crate::impl_rig_tooldyn!(TaskDoneTool);

#[cfg(test)]
mod tests {
    use super::*;
    use rig::tool::ToolDyn;

    #[tokio::test]
    async fn test_task_done_without_details() {
        let tool = TaskDoneTool::new();
        let output = ToolDyn::call(&tool, r#"{"summary":"done"}"#.to_string())
            .await
            .unwrap();

        assert_eq!(output, "Summary: done");
    }

    #[tokio::test]
    async fn test_task_done_with_empty_details_preserves_block() {
        let tool = TaskDoneTool::new();
        let output = ToolDyn::call(&tool, r#"{"summary":"done","details":""}"#.to_string())
            .await
            .unwrap();

        assert_eq!(output, "Summary: done\n\nDetails:\n");
    }

    #[tokio::test]
    async fn test_task_done_definition_contract() {
        let tool = TaskDoneTool::new();
        let definition = ToolDyn::definition(&tool, String::new()).await;

        assert_eq!(definition.name, "task_done");
        assert_eq!(
            definition.description,
            "Mark a task as completed. Use this when you have successfully completed the requested task and want to signal completion."
        );
        assert_eq!(
            definition.parameters,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "summary": {
                        "type": "string",
                        "description": "Summary of what was accomplished"
                    },
                    "details": {
                        "type": "string",
                        "description": "Optional detailed description of the work done"
                    }
                },
                "required": ["summary"]
            })
        );
    }
}
