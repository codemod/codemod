//! Sequential thinking tool

use crate::tools::core::Result;
use crate::tools::core::{ToolCall, ToolResult};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Data structure for a single thought
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThoughtData {
    pub thought: String,
    pub thought_number: i32,
    pub total_thoughts: i32,
    pub next_thought_needed: bool,
    pub is_revision: Option<bool>,
    pub revises_thought: Option<i32>,
    pub branch_from_thought: Option<i32>,
    pub branch_id: Option<String>,
    pub needs_more_thoughts: Option<bool>,
}

/// Tool for structured thinking and reasoning with comprehensive features
pub struct ThinkingTool {
    thought_history: Arc<Mutex<Vec<ThoughtData>>>,
    branches: Arc<Mutex<HashMap<String, Vec<ThoughtData>>>>,
}

impl ThinkingTool {
    pub fn new() -> Self {
        Self {
            thought_history: Arc::new(Mutex::new(Vec::new())),
            branches: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl ThinkingTool {
    fn name(&self) -> &str {
        "sequentialthinking"
    }

    fn description(&self) -> &str {
        "A detailed tool for dynamic and reflective problem-solving through thoughts.\n\
         This tool helps analyze problems through a flexible thinking process that can adapt and evolve.\n\
         Each thought can build on, question, or revise previous insights as understanding deepens.\n\
         \n\
         When to use this tool:\n\
         - Breaking down complex problems into steps\n\
         - Planning and design with room for revision\n\
         - Analysis that might need course correction\n\
         - Problems where the full scope might not be clear initially\n\
         - Problems that require a multi-step solution\n\
         - Tasks that need to maintain context over multiple steps\n\
         - Situations where irrelevant information needs to be filtered out\n\
         \n\
         Key features:\n\
         - You can adjust total_thoughts up or down as you progress\n\
         - You can question or revise previous thoughts\n\
         - You can add more thoughts even after reaching what seemed like the end\n\
         - You can express uncertainty and explore alternative approaches\n\
         - Not every thought needs to build linearly - you can branch or backtrack\n\
         - Generates a solution hypothesis\n\
         - Verifies the hypothesis based on the Chain of Thought steps\n\
         - Repeats the process until satisfied\n\
         - Provides a correct answer\n\
         \n\
         Only set next_thought_needed to false when truly done and a satisfactory answer is reached"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "thought": {
                    "type": "string",
                    "description": "Your current thinking step"
                },
                "next_thought_needed": {
                    "type": "boolean",
                    "description": "Whether another thought step is needed"
                },
                "thought_number": {
                    "type": "integer",
                    "description": "Current thought number. Minimum value is 1.",
                    "minimum": 1
                },
                "total_thoughts": {
                    "type": "integer",
                    "description": "Estimated total thoughts needed. Minimum value is 1.",
                    "minimum": 1
                },
                "is_revision": {
                    "type": "boolean",
                    "description": "Whether this revises previous thinking"
                },
                "revises_thought": {
                    "type": "integer",
                    "description": "Which thought is being reconsidered. Minimum value is 1.",
                    "minimum": 1
                },
                "branch_from_thought": {
                    "type": "integer",
                    "description": "Branching point thought number. Minimum value is 1.",
                    "minimum": 1
                },
                "branch_id": {
                    "type": "string",
                    "description": "Branch identifier"
                },
                "needs_more_thoughts": {
                    "type": "boolean",
                    "description": "If more thoughts are needed"
                }
            },
            "required": ["thought", "next_thought_needed", "thought_number", "total_thoughts"]
        })
    }

    async fn execute(&self, call: ToolCall) -> Result<ToolResult> {
        match self.validate_and_process_thought(&call) {
            Ok(thought_data) => {
                self.add_to_history(thought_data.clone());
                self.handle_branching(&thought_data);

                let response_data = self.create_response_data(&thought_data);

                // Create output that includes both the thought content and status
                let output_with_thought = json!({
                    "thought": thought_data.thought,
                    "Status": response_data
                });

                let output = format!(
                    "Sequential thinking step completed.\n\nThought: {}\n\nStatus:\n{}",
                    thought_data.thought,
                    serde_json::to_string_pretty(&response_data).unwrap_or_default()
                );

                Ok(ToolResult::success(&call.id, &output).with_data(output_with_thought))
            }
            Err(e) => {
                let error_data = json!({
                    "error": e.to_string(),
                    "status": "failed"
                });
                Ok(ToolResult::error(
                    &call.id,
                    format!(
                        "Sequential thinking failed: {}\n\nDetails:\n{}",
                        e,
                        serde_json::to_string_pretty(&error_data).unwrap_or_default()
                    ),
                ))
            }
        }
    }
}

impl ThinkingTool {
    /// Validate input arguments and create ThoughtData
    fn validate_and_process_thought(&self, call: &ToolCall) -> Result<ThoughtData> {
        let thought: String = call
            .get_parameter("thought")
            .map_err(|_| "Invalid thought: must be a string")?;

        let thought_number: i32 = call
            .get_parameter("thought_number")
            .map_err(|_| "Invalid thought_number: must be a number")?;

        let total_thoughts: i32 = call
            .get_parameter("total_thoughts")
            .map_err(|_| "Invalid total_thoughts: must be a number")?;

        let next_thought_needed: bool = call
            .get_parameter("next_thought_needed")
            .map_err(|_| "Invalid next_thought_needed: must be a boolean")?;

        // Validate minimum values
        if thought_number < 1 {
            return Err("thought_number must be at least 1".into());
        }

        if total_thoughts < 1 {
            return Err("total_thoughts must be at least 1".into());
        }

        // Handle optional fields
        let is_revision: Option<bool> = call.get_parameter("is_revision").ok();
        let revises_thought: Option<i32> = call
            .get_parameter("revises_thought")
            .ok()
            .and_then(|v: i32| if v > 0 { Some(v) } else { None });
        let branch_from_thought: Option<i32> = call
            .get_parameter("branch_from_thought")
            .ok()
            .and_then(|v: i32| if v > 0 { Some(v) } else { None });
        let branch_id: Option<String> = call.get_parameter("branch_id").ok();
        let needs_more_thoughts: Option<bool> = call.get_parameter("needs_more_thoughts").ok();

        let mut thought_data = ThoughtData {
            thought,
            thought_number,
            total_thoughts,
            next_thought_needed,
            is_revision,
            revises_thought,
            branch_from_thought,
            branch_id,
            needs_more_thoughts,
        };

        // Adjust total thoughts if current thought number exceeds it
        if thought_data.thought_number > thought_data.total_thoughts {
            thought_data.total_thoughts = thought_data.thought_number;
        }

        Ok(thought_data)
    }

    /// Add thought to history
    fn add_to_history(&self, thought_data: ThoughtData) {
        if let Ok(mut history) = self.thought_history.lock() {
            history.push(thought_data);
        }
    }

    /// Handle branching logic
    fn handle_branching(&self, thought_data: &ThoughtData) {
        if let (Some(_), Some(branch_id)) =
            (&thought_data.branch_from_thought, &thought_data.branch_id)
        {
            if let Ok(mut branches) = self.branches.lock() {
                branches
                    .entry(branch_id.clone())
                    .or_insert_with(Vec::new)
                    .push(thought_data.clone());
            }
        }
    }

    /// Create response data for output
    fn create_response_data(&self, thought_data: &ThoughtData) -> serde_json::Value {
        let branch_keys: Vec<String> = if let Ok(branches) = self.branches.lock() {
            branches.keys().cloned().collect()
        } else {
            Vec::new()
        };

        let history_length = if let Ok(history) = self.thought_history.lock() {
            history.len()
        } else {
            0
        };

        json!({
            "thought_number": thought_data.thought_number,
            "total_thoughts": thought_data.total_thoughts,
            "next_thought_needed": thought_data.next_thought_needed,
            "branches": branch_keys,
            "thought_history_length": history_length
        })
    }
}

impl Default for ThinkingTool {
    fn default() -> Self {
        Self::new()
    }
}

crate::impl_rig_tooldyn!(ThinkingTool);
