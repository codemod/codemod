//! File editing tool

use async_trait::async_trait;
use coro_core::error::Result;
use coro_core::impl_tool_factory;
use coro_core::tools::utils::{
    check_file_exists, create_edit_snippet, expand_tabs, format_with_line_numbers, maybe_truncate,
    run_command, validate_absolute_path, validate_directory_operation,
};
use coro_core::tools::{Tool, ToolCall, ToolExample, ToolResult};
use serde_json::json;
use std::path::Path;

/// Number of lines to show in snippets
const SNIPPET_LINES: usize = 4;

/// Available edit tool commands
const EDIT_TOOL_COMMANDS: &[&str] = &["view", "create", "str_replace", "insert"];

/// Tool for editing files with comprehensive functionality
pub struct EditTool;

impl EditTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &str {
        "str_replace_based_edit_tool"
    }

    fn description(&self) -> &str {
        "Custom editing tool for viewing, creating and editing files\n\
         * State is persistent across command calls and discussions with the user\n\
         * If `path` is a file, `view` displays the result of applying `cat -n`. If `path` is a directory, `view` lists non-hidden files and directories up to 2 levels deep\n\
         * The `create` command cannot be used if the specified `path` already exists as a file !!! If you know that the `path` already exists, please remove it first and then perform the `create` operation!\n\
         * If a `command` generates a long output, it will be truncated and marked with `<response clipped>`\n\
         \n\
         IMPORTANT PATH REQUIREMENT:\n\
         * ALL paths must be ABSOLUTE paths. You MUST construct the full path by combining the [Project root path] from the user's message with the relative file path.\n\
         * Example: If project root is `/home/user/project` and you want to edit `src/main.rs`, use `/home/user/project/src/main.rs`\n\
         * DO NOT use relative paths like `src/main.rs` - they will fail!\n\
         \n\
         Notes for using the `str_replace` command:\n\
         * The `old_str` parameter should match EXACTLY one or more consecutive lines from the original file. Be mindful of whitespaces!\n\
         * If the `old_str` parameter is not unique in the file, the replacement will not be performed. Make sure to include enough context in `old_str` to make it unique\n\
         * The `new_str` parameter should contain the edited lines that should replace the `old_str`"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "enum": ["view", "create", "str_replace", "insert"],
                    "description": "The commands to run. Allowed options are: view, create, str_replace, insert."
                },
                "path": {
                    "type": "string",
                    "description": "Absolute path to file or directory, e.g. `/repo/file.py` or `/repo`."
                },
                "file_text": {
                    "type": "string",
                    "description": "Required parameter of `create` command, with the content of the file to be created."
                },
                "old_str": {
                    "type": "string",
                    "description": "Required parameter of `str_replace` command containing the string in `path` to replace."
                },
                "new_str": {
                    "type": "string",
                    "description": "Optional parameter of `str_replace` command containing the new string (if not given, no string will be added). Required parameter of `insert` command containing the string to insert."
                },
                "insert_line": {
                    "type": "integer",
                    "description": "Required parameter of `insert` command. The `new_str` will be inserted AFTER the line `insert_line` of `path`."
                },
                "view_range": {
                    "type": "array",
                    "items": {"type": "integer"},
                    "description": "Optional parameter of `view` command when `path` points to a file. If none is given, the full file is shown. If provided, the file will be shown in the indicated line number range, e.g. [11, 12] will show lines 11 and 12. Indexing at 1 to start. Setting `[start_line, -1]` shows all lines from `start_line` to the end of the file."
                }
            },
            "required": ["command", "path"]
        })
    }

    async fn execute(&self, call: ToolCall) -> Result<ToolResult> {
        let command: String = call.get_parameter("command")?;
        let path_str: String = call.get_parameter("path")?;
        let path = Path::new(&path_str);

        // Validate path and command
        if let Err(e) = self.validate_path(&command, path) {
            return Ok(ToolResult::error(&call.id, &e.to_string()));
        }

        match command.as_str() {
            "view" => {
                let view_range: Option<Vec<i32>> = call.get_parameter("view_range").ok();
                self.view_handler(&call.id, path, view_range).await
            }
            "create" => {
                let file_text: String = call.get_parameter("file_text").map_err(|_| {
                    "Parameter `file_text` is required and must be a string for command: create"
                })?;
                self.create_handler(&call.id, path, &file_text).await
            }
            "str_replace" => {
                let old_str: String = call.get_parameter("old_str")
                    .map_err(|_| "Parameter `old_str` is required and should be a string for command: str_replace")?;
                let new_str: Option<String> = call.get_parameter("new_str").ok();
                self.str_replace_handler(&call.id, path, &old_str, new_str.as_deref())
                    .await
            }
            "insert" => {
                let insert_line: i32 = call.get_parameter("insert_line").map_err(|_| {
                    "Parameter `insert_line` is required and should be integer for command: insert"
                })?;
                let new_str: String = call
                    .get_parameter("new_str")
                    .map_err(|_| "Parameter `new_str` is required for command: insert")?;
                self.insert_handler(&call.id, path, insert_line, &new_str)
                    .await
            }
            _ => Ok(ToolResult::error(
                &call.id,
                &format!(
                    "Unrecognized command {}. The allowed commands for the {} tool are: {}",
                    command,
                    self.name(),
                    EDIT_TOOL_COMMANDS.join(", ")
                ),
            )),
        }
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![
            ToolExample {
                description: "View a file".to_string(),
                parameters: json!({"command": "view", "path": "/repo/src/main.rs"}),
                expected_result: "File contents with line numbers".to_string(),
            },
            ToolExample {
                description: "View a file with line range".to_string(),
                parameters: json!({"command": "view", "path": "/repo/src/main.rs", "view_range": [10, 20]}),
                expected_result: "File contents from line 10 to 20 with line numbers".to_string(),
            },
            ToolExample {
                description: "Create a new file".to_string(),
                parameters: json!({
                    "command": "create",
                    "path": "/repo/hello.txt",
                    "file_text": "Hello, world!"
                }),
                expected_result: "File created successfully".to_string(),
            },
            ToolExample {
                description: "Replace text in a file".to_string(),
                parameters: json!({
                    "command": "str_replace",
                    "path": "/repo/src/main.rs",
                    "old_str": "println!(\"Hello, world!\");",
                    "new_str": "println!(\"Hello, Rust!\");"
                }),
                expected_result: "Text replaced with snippet showing changes".to_string(),
            },
            ToolExample {
                description: "Insert text after a specific line".to_string(),
                parameters: json!({
                    "command": "insert",
                    "path": "/repo/src/main.rs",
                    "insert_line": 5,
                    "new_str": "    // This is a new comment"
                }),
                expected_result: "Text inserted with snippet showing changes".to_string(),
            },
        ]
    }
}

impl EditTool {
    /// Validate path and command combination
    fn validate_path(&self, command: &str, path: &Path) -> Result<()> {
        validate_absolute_path(path)?;
        check_file_exists(path, command)?;
        validate_directory_operation(path, command)?;
        Ok(())
    }

    /// Handle view command
    async fn view_handler(
        &self,
        call_id: &str,
        path: &Path,
        view_range: Option<Vec<i32>>,
    ) -> Result<ToolResult> {
        if path.is_dir() {
            if view_range.is_some() {
                return Ok(ToolResult::error(
                    call_id,
                    "The `view_range` parameter is not allowed when `path` points to a directory.",
                ));
            }
            return self.view_directory(call_id, path).await;
        }

        self.view_file(call_id, path, view_range).await
    }

    /// View directory contents
    async fn view_directory(&self, call_id: &str, path: &Path) -> Result<ToolResult> {
        let find_cmd = format!("find {} -maxdepth 2 -not -path '*/\\.*'", path.display());
        let (return_code, stdout, stderr) = run_command(&find_cmd, Some(30), None).await?;

        if return_code == 0 && stderr.is_empty() {
            let output = format!(
                "Here's the files and directories up to 2 levels deep in {}, excluding hidden items:\n{}\n",
                path.display(),
                stdout
            );
            Ok(ToolResult::success(call_id, &output))
        } else {
            Ok(ToolResult::error(call_id, &stderr))
        }
    }

    /// View file contents with optional range
    async fn view_file(
        &self,
        call_id: &str,
        path: &Path,
        view_range: Option<Vec<i32>>,
    ) -> Result<ToolResult> {
        let file_content = self.read_file(path)?;
        let init_line = 1;

        let content_to_show = if let Some(range) = view_range {
            if range.len() != 2 {
                return Ok(ToolResult::error(
                    call_id,
                    "Invalid `view_range`. It should be a list of two integers.",
                ));
            }

            let file_lines: Vec<&str> = file_content.lines().collect();
            let n_lines_file = file_lines.len() as i32;
            let (init_line, final_line) = (range[0], range[1]);

            if init_line < 1 || init_line > n_lines_file {
                return Ok(ToolResult::error(call_id, &format!(
                    "Invalid `view_range`: {:?}. Its first element `{}` should be within the range of lines of the file: [1, {}]",
                    range, init_line, n_lines_file
                )));
            }

            if final_line > n_lines_file {
                return Ok(ToolResult::error(call_id, &format!(
                    "Invalid `view_range`: {:?}. Its second element `{}` should be smaller than the number of lines in the file: `{}`",
                    range, final_line, n_lines_file
                )));
            }

            if final_line != -1 && final_line < init_line {
                return Ok(ToolResult::error(call_id, &format!(
                    "Invalid `view_range`: {:?}. Its second element `{}` should be larger or equal than its first `{}`",
                    range, final_line, init_line
                )));
            }

            let start_idx = (init_line - 1) as usize;
            let end_idx = if final_line == -1 {
                file_lines.len()
            } else {
                final_line as usize
            };

            (file_lines[start_idx..end_idx].join("\n"), init_line)
        } else {
            (file_content, init_line)
        };

        let output = self.make_output(
            &content_to_show.0,
            &format!("{}", path.display()),
            content_to_show.1,
        );
        Ok(ToolResult::success(call_id, &output))
    }

    /// Handle create command
    async fn create_handler(
        &self,
        call_id: &str,
        path: &Path,
        file_text: &str,
    ) -> Result<ToolResult> {
        self.write_file(path, file_text)?;
        Ok(ToolResult::success(
            call_id,
            &format!("File created successfully at: {}", path.display()),
        ))
    }

    /// Handle str_replace command
    async fn str_replace_handler(
        &self,
        call_id: &str,
        path: &Path,
        old_str: &str,
        new_str: Option<&str>,
    ) -> Result<ToolResult> {
        let file_content = expand_tabs(&self.read_file(path)?);
        let old_str_expanded = expand_tabs(old_str);
        let new_str_expanded = new_str.map(expand_tabs).unwrap_or_default();

        // Check if old_str is unique in the file
        let occurrences = file_content.matches(&old_str_expanded).count();
        if occurrences == 0 {
            return Ok(ToolResult::error(
                call_id,
                &format!(
                    "No replacement was performed, old_str `{}` did not appear verbatim in {}.",
                    old_str,
                    path.display()
                ),
            ));
        } else if occurrences > 1 {
            let file_lines: Vec<&str> = file_content.lines().collect();
            let lines: Vec<usize> = file_lines
                .iter()
                .enumerate()
                .filter_map(|(idx, line)| {
                    if line.contains(&old_str_expanded) {
                        Some(idx + 1)
                    } else {
                        None
                    }
                })
                .collect();
            return Ok(ToolResult::error(call_id, &format!(
                "No replacement was performed. Multiple occurrences of old_str `{}` in lines {:?}. Please ensure it is unique",
                old_str, lines
            )));
        }

        // Replace old_str with new_str
        let new_file_content = file_content.replace(&old_str_expanded, &new_str_expanded);
        self.write_file(path, &new_file_content)?;

        // Create a snippet of the edited section
        let replacement_line = file_content
            .split(&old_str_expanded)
            .next()
            .unwrap()
            .lines()
            .count();
        let snippet = create_edit_snippet(&new_file_content, replacement_line, SNIPPET_LINES);

        let success_msg = format!(
            "The file {} has been edited. {}\nReview the changes and make sure they are as expected. Edit the file again if necessary.",
            path.display(),
            self.make_output(&snippet, &format!("a snippet of {}", path.display()), (replacement_line.saturating_sub(SNIPPET_LINES) + 1) as i32)
        );

        Ok(ToolResult::success(call_id, &success_msg))
    }

    /// Handle insert command
    async fn insert_handler(
        &self,
        call_id: &str,
        path: &Path,
        insert_line: i32,
        new_str: &str,
    ) -> Result<ToolResult> {
        let file_text = expand_tabs(&self.read_file(path)?);
        let new_str_expanded = expand_tabs(new_str);
        let mut file_text_lines: Vec<&str> = file_text.lines().collect();
        let n_lines_file = file_text_lines.len() as i32;

        if insert_line < 0 || insert_line > n_lines_file {
            return Ok(ToolResult::error(call_id, &format!(
                "Invalid `insert_line` parameter: {}. It should be within the range of lines of the file: [0, {}]",
                insert_line, n_lines_file
            )));
        }

        let new_str_lines: Vec<&str> = new_str_expanded.lines().collect();
        let insert_idx = insert_line as usize;

        // Insert new lines
        for (i, line) in new_str_lines.iter().enumerate() {
            file_text_lines.insert(insert_idx + i, line);
        }

        let new_file_text = file_text_lines.join("\n");
        self.write_file(path, &new_file_text)?;

        let snippet = create_edit_snippet(&new_file_text, insert_idx, SNIPPET_LINES);
        let success_msg = format!(
            "The file {} has been edited. {}\nReview the changes and make sure they are as expected (correct indentation, no duplicate lines, etc). Edit the file again if necessary.",
            path.display(),
            self.make_output(&snippet, "a snippet of the edited file", (insert_idx.saturating_sub(SNIPPET_LINES) + 1) as i32)
        );

        Ok(ToolResult::success(call_id, &success_msg))
    }

    /// Read file content
    fn read_file(&self, path: &Path) -> Result<String> {
        std::fs::read_to_string(path)
            .map_err(|e| format!("Ran into {} while trying to read {}", e, path.display()).into())
    }

    /// Write file content
    fn write_file(&self, path: &Path, content: &str) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "Failed to create parent directories for {}: {}",
                    path.display(),
                    e
                )
            })?;
        }

        std::fs::write(path, content).map_err(|e| {
            format!("Ran into {} while trying to write to {}", e, path.display()).into()
        })
    }

    /// Generate output for the CLI based on the content of a file
    fn make_output(&self, file_content: &str, file_descriptor: &str, init_line: i32) -> String {
        let truncated_content = maybe_truncate(file_content, None);
        let formatted_content = format_with_line_numbers(&truncated_content, init_line as usize);
        format!(
            "Here's the result of running `cat -n` on {}:\n{}\n",
            file_descriptor, formatted_content
        )
    }
}

impl Default for EditTool {
    fn default() -> Self {
        Self::new()
    }
}

impl_tool_factory!(
    EditToolFactory,
    EditTool,
    "str_replace_based_edit_tool",
    "Edit files by viewing, creating, or replacing text content"
);
