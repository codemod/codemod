//! Core tool types and helpers for codemod-ai.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

/// Result type used by codemod-ai tools.
pub type Result<T> = std::result::Result<T, Error>;

/// Error type used by codemod-ai tools.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct Error {
    message: String,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl From<&str> for Error {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for Error {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::new(value.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self::new(value.to_string())
    }
}

impl From<tokio::time::error::Elapsed> for Error {
    fn from(value: tokio::time::error::Elapsed) -> Self {
        Self::new(value.to_string())
    }
}

impl From<rusqlite::Error> for Error {
    fn from(value: rusqlite::Error) -> Self {
        Self::new(value.to_string())
    }
}

impl From<tree_sitter::LanguageError> for Error {
    fn from(value: tree_sitter::LanguageError) -> Self {
        Self::new(value.to_string())
    }
}

impl From<ignore::Error> for Error {
    fn from(value: ignore::Error) -> Self {
        Self::new(value.to_string())
    }
}

/// A call to a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Unique identifier for this tool call.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Tool parameters.
    pub parameters: serde_json::Value,
}

/// Result of a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// ID of the originating tool call.
    pub tool_call_id: String,
    /// Whether execution succeeded.
    pub success: bool,
    /// Human-readable result content.
    pub content: String,
    /// Optional structured data payload.
    pub data: Option<serde_json::Value>,
}

impl ToolCall {
    /// Create a tool call with an auto-generated call id.
    pub fn new<S: Into<String>>(name: S, parameters: serde_json::Value) -> Self {
        static TOOL_CALL_COUNTER: AtomicU64 = AtomicU64::new(1);

        Self {
            id: format!(
                "tool-call-{}",
                TOOL_CALL_COUNTER.fetch_add(1, Ordering::Relaxed)
            ),
            name: name.into(),
            parameters,
        }
    }

    /// Get a required parameter.
    pub fn get_parameter<T>(&self, key: &str) -> Result<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        let value = self
            .parameters
            .get(key)
            .ok_or_else(|| Error::new(format!("Missing parameter: {}", key)))?;

        serde_json::from_value(value.clone())
            .map_err(|_| Error::new(format!("Invalid parameter type for: {}", key)))
    }

    /// Get an optional parameter with default fallback.
    pub fn get_parameter_or<T>(&self, key: &str, default: T) -> T
    where
        T: for<'de> Deserialize<'de> + Clone,
    {
        self.get_parameter(key).unwrap_or(default)
    }
}

impl ToolResult {
    /// Build a successful tool result.
    pub fn success<ID: Into<String>, C: Into<String>>(tool_call_id: ID, content: C) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            success: true,
            content: content.into(),
            data: None,
        }
    }

    /// Build an error tool result.
    pub fn error<ID: Into<String>, E: Into<String>>(tool_call_id: ID, error: E) -> Self {
        Self {
            tool_call_id: tool_call_id.into(),
            success: false,
            content: format!("Error: {}", error.into()),
            data: None,
        }
    }

    /// Attach structured data payload.
    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }
}

#[macro_export]
macro_rules! impl_rig_tooldyn {
    ($tool:ty) => {
        impl rig::tool::ToolDyn for $tool {
            fn name(&self) -> String {
                self.name().to_string()
            }

            fn definition(
                &self,
                prompt: String,
            ) -> rig::wasm_compat::WasmBoxedFuture<'_, rig::completion::ToolDefinition> {
                let _ = prompt;
                let name = self.name().to_string();
                let description = self.description().to_string();
                let parameters = self.parameters_schema();

                Box::pin(async move {
                    rig::completion::ToolDefinition {
                        name,
                        description,
                        parameters,
                    }
                })
            }

            fn call(
                &self,
                args: String,
            ) -> rig::wasm_compat::WasmBoxedFuture<
                '_,
                std::result::Result<String, rig::tool::ToolError>,
            > {
                Box::pin(async move {
                    let parameters: serde_json::Value =
                        serde_json::from_str(&args).map_err(rig::tool::ToolError::JsonError)?;

                    let call = $crate::tools::core::ToolCall::new(self.name(), parameters);
                    let result = self
                        .execute(call)
                        .await
                        .map_err(|e| rig::tool::ToolError::ToolCallError(Box::new(e)))?;

                    Ok(result.content)
                })
            }
        }
    };
}
