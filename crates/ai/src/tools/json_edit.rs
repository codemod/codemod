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

    /// Parse JSONPath into a list of keys
    fn parse_json_path(&self, json_path: &str) -> Result<Vec<String>> {
        if json_path == "$" {
            return Ok(vec![]);
        }

        if !json_path.starts_with('$') {
            return Err("JSONPath must start with '$'".into());
        }

        let path = &json_path[1..];
        let mut keys = Vec::new();
        let mut chars = path.chars().peekable();

        while chars.peek().is_some() {
            match chars.next() {
                Some('.') => {
                    // Dot notation: .key
                    let mut key = String::new();
                    while let Some(&ch) = chars.peek() {
                        if ch == '.' || ch == '[' {
                            break;
                        }
                        key.push(ch);
                        chars.next();
                    }
                    if !key.is_empty() {
                        keys.push(key);
                    }
                }
                Some('[') => {
                    // bracket notation: ["key"] or ['key'] or [0]
                    let quote_char = chars.peek().copied();

                    if quote_char == Some('"') || quote_char == Some('\'') {
                        // string key: ["key"] or ['key']
                        chars.next(); // consume the quote
                        let quote = quote_char.unwrap();
                        let mut key = String::new();
                        let mut escaped = false;

                        for ch in chars.by_ref() {
                            if escaped {
                                key.push(ch);
                                escaped = false;
                            } else if ch == '\\' {
                                escaped = true;
                            } else if ch == quote {
                                break;
                            } else {
                                key.push(ch);
                            }
                        }

                        // consume the closing bracket
                        if chars.peek() == Some(&']') {
                            chars.next();
                        }

                        keys.push(key);
                    } else {
                        // numeric index: [0]
                        let mut index_str = String::new();
                        while let Some(&ch) = chars.peek() {
                            if ch == ']' {
                                chars.next();
                                break;
                            }
                            index_str.push(ch);
                            chars.next();
                        }
                        keys.push(index_str);
                    }
                }
                _ => {
                    return Err("Invalid JSONPath syntax".into());
                }
            }
        }

        Ok(keys)
    }

    /// Set value at JSONPath (simplified implementation)
    fn set_value_at_path(&self, data: &mut Value, json_path: &str, value: Value) -> Result<()> {
        // Handle root replacement
        if json_path == "$" {
            *data = value;
            return Ok(());
        }

        let path_parts = self.parse_json_path(json_path)?;
        if path_parts.is_empty() {
            *data = value;
            return Ok(());
        }

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
                    if !map.contains_key(part) {
                        map.insert(part.to_string(), Value::Object(serde_json::Map::new()));
                    }
                    current = map.get_mut(part).unwrap();
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
        let path_parts = self.parse_json_path(json_path)?;

        if path_parts.is_empty() {
            return Err("Cannot remove root element".into());
        }

        let mut current = data;

        // Navigate to parent
        for part in &path_parts[..path_parts.len() - 1] {
            if let Value::Object(ref mut map) = current {
                current = map
                    .get_mut(part)
                    .ok_or_else(|| format!("Path '{}' not found", part))?;
            } else {
                return Err(format!("Cannot navigate to '{}' on non-object", part).into());
            }
        }

        // Remove the final key
        let final_key = path_parts.last().unwrap();
        if let Value::Object(ref mut map) = current {
            if map.remove(final_key).is_none() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use tokio::fs;

    fn create_tool_call(
        id: &str,
        operation: &str,
        file_path: &str,
        json_path: Option<&str>,
        value: Option<serde_json::Value>,
    ) -> ToolCall {
        let mut params = json!({
            "operation": operation,
            "file_path": file_path,
        });

        if let Some(path) = json_path {
            params["json_path"] = json!(path);
        }

        if let Some(val) = value {
            params["value"] = val;
        }

        ToolCall {
            id: id.to_string(),
            name: "json_edit_tool".to_string(),
            parameters: params,
            metadata: None,
        }
    }

    async fn create_temp_json_file(content: serde_json::Value) -> NamedTempFile {
        let temp_file = NamedTempFile::new().unwrap();
        let json_str = serde_json::to_string_pretty(&content).unwrap();
        fs::write(temp_file.path(), json_str).await.unwrap();
        temp_file
    }

    #[tokio::test]
    async fn test_view_entire_file() {
        let tool = JsonEditTool::new();
        let test_data = json!({
            "name": "test",
            "version": "1.0.0",
            "config": {
                "enabled": true
            }
        });

        let temp_file = create_temp_json_file(test_data).await;
        let call = create_tool_call(
            "test-1",
            "view",
            temp_file.path().to_str().unwrap(),
            None,
            None,
        );

        let result = tool.execute(call).await.unwrap();
        assert!(result.success);
        assert!(result.content.contains("\"name\""));
        assert!(result.content.contains("\"test\""));
    }

    #[tokio::test]
    async fn test_view_with_simple_jsonpath() {
        let tool = JsonEditTool::new();
        let test_data = json!({
            "database": {
                "host": "localhost",
                "port": 5432
            }
        });

        let temp_file = create_temp_json_file(test_data).await;
        let call = create_tool_call(
            "test-2",
            "view",
            temp_file.path().to_str().unwrap(),
            Some("$.database.host"),
            None,
        );

        let result = tool.execute(call).await.unwrap();
        assert!(result.success);
        assert!(result.content.contains("localhost"));
    }

    #[tokio::test]
    async fn test_set_simple_dot_notation() {
        let tool = JsonEditTool::new();
        let test_data = json!({
            "config": {
                "port": 3000
            }
        });

        let temp_file = create_temp_json_file(test_data).await;
        let call = create_tool_call(
            "test-3",
            "set",
            temp_file.path().to_str().unwrap(),
            Some("$.config.port"),
            Some(json!(8080)),
        );

        let result = tool.execute(call).await.unwrap();
        assert!(result.success, "Result: {:?}", result);

        let content = fs::read_to_string(temp_file.path()).await.unwrap();
        let updated_data: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(updated_data["config"]["port"], 8080);
    }

    #[tokio::test]
    async fn test_set_with_bracket_notation() {
        let tool = JsonEditTool::new();
        let test_data = json!({
            "bulk-actions-dropdown": {
                "accessibility": {
                    "toggle-menu": "Old value"
                }
            }
        });

        let temp_file = create_temp_json_file(test_data).await;

        let call = create_tool_call(
            "test-4",
            "set",
            temp_file.path().to_str().unwrap(),
            Some("$[\"bulk-actions-dropdown\"][\"accessibility\"][\"toggle-menu\"]"),
            Some(json!("Toggle bulk select menu")),
        );

        let _ = tool.execute(call).await.unwrap();

        let content = fs::read_to_string(temp_file.path()).await.unwrap();
        let updated_data: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert_eq!(
            updated_data["bulk-actions-dropdown"]["accessibility"]["toggle-menu"],
            "Toggle bulk select menu",
            "The nested structure should be preserved, not flattened into a single key"
        );
    }

    #[tokio::test]
    async fn test_set_with_mixed_notation() {
        let tool = JsonEditTool::new();
        let test_data = json!({
            "my-config": {
                "database": {
                    "host": "localhost"
                }
            }
        });

        let temp_file = create_temp_json_file(test_data).await;

        // Mix of bracket and dot notation
        let call = create_tool_call(
            "test-5",
            "set",
            temp_file.path().to_str().unwrap(),
            Some("$[\"my-config\"].database.host"),
            Some(json!("remote-server")),
        );

        let _result = tool.execute(call).await.unwrap();
        let content = fs::read_to_string(temp_file.path()).await.unwrap();
        let updated_data: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert_eq!(
            updated_data["my-config"]["database"]["host"],
            "remote-server"
        );
    }

    #[tokio::test]
    async fn test_add_new_property() {
        let tool = JsonEditTool::new();
        let test_data = json!({
            "config": {
                "existing": "value"
            }
        });

        let temp_file = create_temp_json_file(test_data).await;
        let call = create_tool_call(
            "test-6",
            "add",
            temp_file.path().to_str().unwrap(),
            Some("$.config.new_property"),
            Some(json!("new value")),
        );

        let result = tool.execute(call).await.unwrap();
        assert!(result.success);

        let content = fs::read_to_string(temp_file.path()).await.unwrap();
        let updated_data: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(updated_data["config"]["new_property"], "new value");
        assert_eq!(updated_data["config"]["existing"], "value");
    }

    #[tokio::test]
    async fn test_add_nested_path_that_doesnt_exist() {
        let tool = JsonEditTool::new();
        let test_data = json!({
            "config": {}
        });

        let temp_file = create_temp_json_file(test_data).await;
        let call = create_tool_call(
            "test-7",
            "add",
            temp_file.path().to_str().unwrap(),
            Some("$.config.database.connection.pool"),
            Some(json!(10)),
        );

        let result = tool.execute(call).await.unwrap();
        assert!(result.success);

        let content = fs::read_to_string(temp_file.path()).await.unwrap();
        let updated_data: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(updated_data["config"]["database"]["connection"]["pool"], 10);
    }

    #[tokio::test]
    async fn test_remove_property() {
        let tool = JsonEditTool::new();
        let test_data = json!({
            "config": {
                "to_keep": "keep this",
                "to_remove": "remove this"
            }
        });

        let temp_file = create_temp_json_file(test_data).await;
        let call = create_tool_call(
            "test-8",
            "remove",
            temp_file.path().to_str().unwrap(),
            Some("$.config.to_remove"),
            None,
        );

        let result = tool.execute(call).await.unwrap();
        assert!(result.success);

        let content = fs::read_to_string(temp_file.path()).await.unwrap();
        let updated_data: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(updated_data["config"]["to_keep"], "keep this");
        assert!(updated_data["config"]["to_remove"].is_null());
    }

    #[tokio::test]
    async fn test_remove_with_bracket_notation() {
        let tool = JsonEditTool::new();
        let test_data = json!({
            "my-key": {
                "nested-key": "value"
            }
        });

        let temp_file = create_temp_json_file(test_data).await;
        let call = create_tool_call(
            "test-9",
            "remove",
            temp_file.path().to_str().unwrap(),
            Some("$[\"my-key\"][\"nested-key\"]"),
            None,
        );

        let _result = tool.execute(call).await.unwrap();
        let content = fs::read_to_string(temp_file.path()).await.unwrap();
        let updated_data: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert!(updated_data["my-key"].is_object());
        assert!(updated_data["my-key"]["nested-key"].is_null());
    }

    #[tokio::test]
    async fn test_set_root_value() {
        let tool = JsonEditTool::new();
        let test_data = json!({"old": "data"});

        let temp_file = create_temp_json_file(test_data).await;
        let call = create_tool_call(
            "test-10",
            "set",
            temp_file.path().to_str().unwrap(),
            Some("$"),
            Some(json!({"new": "data"})),
        );

        let result = tool.execute(call).await.unwrap();
        assert!(result.success);

        let content = fs::read_to_string(temp_file.path()).await.unwrap();
        let updated_data: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(updated_data["new"], "data");
        assert!(updated_data["old"].is_null());
    }

    #[tokio::test]
    async fn test_error_on_nonexistent_file() {
        let tool = JsonEditTool::new();
        let call = create_tool_call("test-11", "view", "/nonexistent/file.json", None, None);

        let result = tool.execute(call).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not exist"));
    }

    #[tokio::test]
    async fn test_error_on_invalid_json() {
        let temp_file = NamedTempFile::new().unwrap();
        fs::write(temp_file.path(), "not valid json").await.unwrap();

        let tool = JsonEditTool::new();
        let call = create_tool_call(
            "test-12",
            "view",
            temp_file.path().to_str().unwrap(),
            None,
            None,
        );

        let result = tool.execute(call).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid JSON"));
    }

    #[tokio::test]
    async fn test_error_on_invalid_jsonpath() {
        let tool = JsonEditTool::new();
        let test_data = json!({"key": "value"});

        let temp_file = create_temp_json_file(test_data).await;
        let call = create_tool_call(
            "test-13",
            "set",
            temp_file.path().to_str().unwrap(),
            Some("invalid_path"),
            Some(json!("new value")),
        );

        let result = tool.execute(call).await.unwrap();
        assert!(!result.success);
        assert!(
            result.content.contains("JSONPath must start with")
                || result.content.contains("Invalid JSONPath")
        );
    }

    #[tokio::test]
    async fn test_complex_nested_structure() {
        let tool = JsonEditTool::new();
        let test_data = json!({
            "translations": {
                "en": {
                    "buttons": {
                        "submit": "Submit",
                        "cancel": "Cancel"
                    }
                }
            }
        });

        let temp_file = create_temp_json_file(test_data).await;
        let call = create_tool_call(
            "test-14",
            "set",
            temp_file.path().to_str().unwrap(),
            Some("$.translations.en.buttons.submit"),
            Some(json!("Send")),
        );

        let result = tool.execute(call).await.unwrap();
        assert!(result.success);

        let content = fs::read_to_string(temp_file.path()).await.unwrap();
        let updated_data: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            updated_data["translations"]["en"]["buttons"]["submit"],
            "Send"
        );
        assert_eq!(
            updated_data["translations"]["en"]["buttons"]["cancel"],
            "Cancel"
        );
    }

    #[tokio::test]
    async fn test_user_reported_issue_bracket_notation_keys_with_hyphens() {
        // This test specifically addresses the user's reported issue where
        // keys like $["bulk-actions-dropdown"]["accessibility"]["toggle-menu"]
        // were being created as a single flattened key instead of nested objects
        let tool = JsonEditTool::new();
        let test_data = json!({});

        let temp_file = create_temp_json_file(test_data).await;
        let call = create_tool_call(
            "test-15",
            "set",
            temp_file.path().to_str().unwrap(),
            Some("$[\"bulk-actions-dropdown\"][\"accessibility\"][\"toggle-menu\"]"),
            Some(json!("Toggle Zap bulk select menu")),
        );

        let result = tool.execute(call).await.unwrap();
        assert!(result.success, "Operation should succeed");

        // Verify the JSON structure is properly nested, not flattened
        let content = fs::read_to_string(temp_file.path()).await.unwrap();
        let updated_data: serde_json::Value = serde_json::from_str(&content).unwrap();

        // The structure should be nested objects, not a single key
        assert!(updated_data.is_object(), "Root should be an object");
        assert!(
            updated_data.get("bulk-actions-dropdown").is_some(),
            "Should have 'bulk-actions-dropdown' key"
        );
        assert!(
            updated_data["bulk-actions-dropdown"].is_object(),
            "'bulk-actions-dropdown' should be an object"
        );
        assert!(
            updated_data["bulk-actions-dropdown"]["accessibility"].is_object(),
            "'accessibility' should be an object"
        );
        assert_eq!(
            updated_data["bulk-actions-dropdown"]["accessibility"]["toggle-menu"],
            "Toggle Zap bulk select menu",
            "Final value should be set correctly"
        );

        // Make sure the problematic flattened key doesn't exist
        let flattened_key = "[\"bulk-actions-dropdown\"][\"accessibility\"][\"toggle-menu\"]";
        assert!(
            updated_data.get(flattened_key).is_none(),
            "Should NOT have a flattened key like '{}'",
            flattened_key
        );
    }
}
