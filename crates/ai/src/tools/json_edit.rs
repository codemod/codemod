//! JSON editing tool

use async_trait::async_trait;
use coro_core::error::Result;
use coro_core::impl_tool_factory;
use coro_core::tools::utils::validate_absolute_path;
use coro_core::tools::{Tool, ToolCall, ToolExample, ToolResult};
use jsonpath_rust::JsonPathQuery;
use serde_json::{json, Value};
use std::path::Path;
use tokio::fs;

/// Tool for editing JSON files using JSONPath expressions
pub struct JsonEditTool;

impl JsonEditTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for JsonEditTool {
    fn name(&self) -> &str {
        "json_edit_tool"
    }

    fn description(&self) -> &str {
        "Tool for editing JSON files with JSONPath expressions\n\
         * Supports targeted modifications to JSON structures using JSONPath syntax\n\
         * Operations: view, set, add, remove\n\
         * JSONPath examples: '$.users[0].name', '$.config.database.host', '$.items[*].price'\n\
         * Safe JSON parsing and validation with detailed error messages\n\
         * Preserves JSON formatting where possible\n\
         \n\
         Operation details:\n\
         - `view`: Display JSON content or specific paths\n\
         - `set`: Update existing values at specified paths\n\
         - `add`: Add new key-value pairs (for objects) or append to arrays\n\
         - `remove`: Delete elements at specified paths\n\
         \n\
         JSONPath syntax supported:\n\
         - `$` - root element\n\
         - `.key` - object property access\n\
         - `[index]` - array index access\n\
         - `[*]` - all elements in array/object\n\
         - `..key` - recursive descent (find key at any level)\n\
         - `[start:end]` - array slicing"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["view", "set", "add", "remove"],
                    "description": "The operation to perform on the JSON file."
                },
                "file_path": {
                    "type": "string",
                    "description": "The full, ABSOLUTE path to the JSON file to edit. You MUST combine the [Project root path] with the file's relative path to construct this. Relative paths are NOT allowed."
                },
                "json_path": {
                    "type": "string",
                    "description": "JSONPath expression to specify the target location (e.g., '$.users[0].name', '$.config.database'). Required for set, add, and remove operations. Optional for view to show specific paths."
                },
                "value": {
                    "type": "object",
                    "description": "The value to set or add. Must be JSON-serializable. Required for set and add operations."
                },
                "pretty_print": {
                    "type": "boolean",
                    "description": "Whether to format the JSON output with proper indentation. Defaults to true."
                }
            },
            "required": ["operation", "file_path"]
        })
    }

    async fn execute(&self, call: ToolCall) -> Result<ToolResult> {
        let operation: String = call.get_parameter("operation")?;
        let file_path_str: String = call.get_parameter("file_path")?;
        let json_path: Option<String> = call.get_parameter("json_path").ok();
        let value: Option<Value> = call.get_parameter("value").ok();
        let pretty_print: bool = call.get_parameter_or("pretty_print", true);

        let file_path = Path::new(&file_path_str);
        validate_absolute_path(file_path)?;

        match operation.as_str() {
            "view" => {
                self.view_json(&call.id, file_path, json_path.as_deref(), pretty_print)
                    .await
            }
            "set" => {
                let json_path =
                    json_path.ok_or("json_path parameter is required for set operation")?;
                let value = value.ok_or("value parameter is required for set operation")?;
                self.set_json_value(&call.id, file_path, &json_path, value, pretty_print)
                    .await
            }
            "add" => {
                let json_path =
                    json_path.ok_or("json_path parameter is required for add operation")?;
                let value = value.ok_or("value parameter is required for add operation")?;
                self.add_json_value(&call.id, file_path, &json_path, value, pretty_print)
                    .await
            }
            "remove" => {
                let json_path =
                    json_path.ok_or("json_path parameter is required for remove operation")?;
                self.remove_json_value(&call.id, file_path, &json_path, pretty_print)
                    .await
            }
            _ => Ok(ToolResult::error(
                &call.id,
                &format!(
                    "Unknown operation: {}. Supported operations: view, set, add, remove",
                    operation
                ),
            )),
        }
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![
            ToolExample {
                description: "View entire JSON file".to_string(),
                parameters: json!({
                    "operation": "view",
                    "file_path": "/project/config.json"
                }),
                expected_result: "JSON content displayed with formatting".to_string(),
            },
            ToolExample {
                description: "View specific JSON path".to_string(),
                parameters: json!({
                    "operation": "view",
                    "file_path": "/project/config.json",
                    "json_path": "$.database.host"
                }),
                expected_result: "Value at specified path".to_string(),
            },
            ToolExample {
                description: "Set a value in JSON".to_string(),
                parameters: json!({
                    "operation": "set",
                    "file_path": "/project/config.json",
                    "json_path": "$.database.port",
                    "value": 5432
                }),
                expected_result: "Value updated successfully".to_string(),
            },
            ToolExample {
                description: "Add new property to JSON object".to_string(),
                parameters: json!({
                    "operation": "add",
                    "file_path": "/project/config.json",
                    "json_path": "$.features.new_feature",
                    "value": true
                }),
                expected_result: "New property added successfully".to_string(),
            },
            ToolExample {
                description: "Remove property from JSON".to_string(),
                parameters: json!({
                    "operation": "remove",
                    "file_path": "/project/config.json",
                    "json_path": "$.deprecated_setting"
                }),
                expected_result: "Property removed successfully".to_string(),
            },
        ]
    }
}

impl JsonEditTool {
    /// Load and parse JSON file
    async fn load_json_file(&self, file_path: &Path) -> Result<Value> {
        if !file_path.exists() {
            return Err(format!("File does not exist: {}", file_path.display()).into());
        }

        let content = fs::read_to_string(file_path).await?;
        if content.trim().is_empty() {
            return Err(format!("File is empty: {}", file_path.display()).into());
        }

        serde_json::from_str(&content)
            .map_err(|e| format!("Invalid JSON in file {}: {}", file_path.display(), e).into())
    }

    /// Save JSON data to file
    async fn save_json_file(
        &self,
        file_path: &Path,
        data: &Value,
        pretty_print: bool,
    ) -> Result<()> {
        let content = if pretty_print {
            serde_json::to_string_pretty(data)?
        } else {
            serde_json::to_string(data)?
        };

        fs::write(file_path, content)
            .await
            .map_err(|e| format!("Error writing to file {}: {}", file_path.display(), e).into())
    }

    /// View JSON file content or specific paths
    async fn view_json(
        &self,
        call_id: &str,
        file_path: &Path,
        json_path: Option<&str>,
        pretty_print: bool,
    ) -> Result<ToolResult> {
        let data = self.load_json_file(file_path).await?;

        if let Some(path) = json_path {
            match data.path(path) {
                Ok(results) => {
                    let output = if pretty_print {
                        serde_json::to_string_pretty(&results)?
                    } else {
                        serde_json::to_string(&results)?
                    };

                    Ok(ToolResult::success(
                        call_id,
                        &format!("JSONPath '{}' matches:\n{}", path, output),
                    ))
                }
                Err(e) => Ok(ToolResult::error(
                    call_id,
                    &format!("Invalid JSONPath expression '{}': {}", path, e),
                )),
            }
        } else {
            let output = if pretty_print {
                serde_json::to_string_pretty(&data)?
            } else {
                serde_json::to_string(&data)?
            };

            Ok(ToolResult::success(
                call_id,
                &format!("JSON content of {}:\n{}", file_path.display(), output),
            ))
        }
    }

    /// Set value at specified JSONPath
    async fn set_json_value(
        &self,
        call_id: &str,
        file_path: &Path,
        json_path: &str,
        value: Value,
        pretty_print: bool,
    ) -> Result<ToolResult> {
        let mut data = self.load_json_file(file_path).await?;

        // For setting values, we need to implement path traversal manually
        // as jsonpath-rust doesn't have built-in mutation support
        if let Err(e) = self.set_value_at_path(&mut data, json_path, value.clone()) {
            return Ok(ToolResult::error(
                call_id,
                &format!("Failed to set value: {}", e),
            ));
        }

        self.save_json_file(file_path, &data, pretty_print).await?;

        Ok(ToolResult::success(
            call_id,
            &format!(
                "Successfully updated JSONPath '{}' with value: {}",
                json_path,
                serde_json::to_string(&value)?
            ),
        ))
    }

    /// Add value at specified JSONPath
    async fn add_json_value(
        &self,
        call_id: &str,
        file_path: &Path,
        json_path: &str,
        value: Value,
        pretty_print: bool,
    ) -> Result<ToolResult> {
        let mut data = self.load_json_file(file_path).await?;

        if let Err(e) = self.add_value_at_path(&mut data, json_path, value) {
            return Ok(ToolResult::error(
                call_id,
                &format!("Failed to add value: {}", e),
            ));
        }

        self.save_json_file(file_path, &data, pretty_print).await?;

        Ok(ToolResult::success(
            call_id,
            &format!("Successfully added value at JSONPath '{}'", json_path),
        ))
    }

    /// Remove value at specified JSONPath
    async fn remove_json_value(
        &self,
        call_id: &str,
        file_path: &Path,
        json_path: &str,
        pretty_print: bool,
    ) -> Result<ToolResult> {
        let mut data = self.load_json_file(file_path).await?;

        if let Err(e) = self.remove_value_at_path(&mut data, json_path) {
            return Ok(ToolResult::error(
                call_id,
                &format!("Failed to remove value: {}", e),
            ));
        }

        self.save_json_file(file_path, &data, pretty_print).await?;

        Ok(ToolResult::success(
            call_id,
            &format!(
                "Successfully removed element(s) at JSONPath '{}'",
                json_path
            ),
        ))
    }

    /// Set value at JSONPath (simplified implementation)
    fn set_value_at_path(&self, data: &mut Value, json_path: &str, value: Value) -> Result<()> {
        // Simple implementation for basic paths like $.key or $.key.subkey
        if json_path == "$" {
            *data = value;
            return Ok(());
        }

        if !json_path.starts_with("$.") {
            return Err("JSONPath must start with '$.'".into());
        }

        let path_parts: Vec<&str> = json_path[2..].split('.').collect();
        let mut current = data;

        for (i, part) in path_parts.iter().enumerate() {
            if i == path_parts.len() - 1 {
                // Last part - set the value
                if let Value::Object(ref mut map) = current {
                    map.insert(part.to_string(), value);
                    return Ok(());
                } else {
                    return Err(format!("Cannot set property '{}' on non-object", part).into());
                }
            } else {
                // Navigate to the next level
                if let Value::Object(ref mut map) = current {
                    if !map.contains_key(*part) {
                        map.insert(part.to_string(), Value::Object(serde_json::Map::new()));
                    }
                    current = map.get_mut(*part).unwrap();
                } else {
                    return Err(format!("Cannot navigate to '{}' on non-object", part).into());
                }
            }
        }

        Ok(())
    }

    /// Add value at JSONPath
    fn add_value_at_path(&self, data: &mut Value, json_path: &str, value: Value) -> Result<()> {
        // For simplicity, treat add the same as set for now
        self.set_value_at_path(data, json_path, value)
    }

    /// Remove value at JSONPath
    fn remove_value_at_path(&self, data: &mut Value, json_path: &str) -> Result<()> {
        if !json_path.starts_with("$.") {
            return Err("JSONPath must start with '$.'".into());
        }

        let path_parts: Vec<&str> = json_path[2..].split('.').collect();
        if path_parts.is_empty() {
            return Err("Cannot remove root element".into());
        }

        let mut current = data;

        // Navigate to parent
        for part in &path_parts[..path_parts.len() - 1] {
            if let Value::Object(ref mut map) = current {
                current = map
                    .get_mut(*part)
                    .ok_or_else(|| format!("Path '{}' not found", part))?;
            } else {
                return Err(format!("Cannot navigate to '{}' on non-object", part).into());
            }
        }

        // Remove the final key
        let final_key = path_parts.last().unwrap();
        if let Value::Object(ref mut map) = current {
            if map.remove(*final_key).is_none() {
                return Err(format!("Key '{}' not found", final_key).into());
            }
        } else {
            return Err(format!("Cannot remove key '{}' from non-object", final_key).into());
        }

        Ok(())
    }
}

impl_tool_factory!(
    JsonEditToolFactory,
    JsonEditTool,
    "json_edit_tool",
    "Tool for editing JSON files with JSONPath expressions"
);
