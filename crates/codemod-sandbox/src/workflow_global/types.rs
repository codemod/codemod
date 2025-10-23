use thiserror::Error;

#[derive(Error, Debug)]
pub enum WorkflowGlobalError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Environment variable error: {0}")]
    EnvVar(String),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Variable not found: {0}")]
    NotFound(String),
}

#[derive(Debug, Clone)]
pub struct Variable {
    pub name: String,
    pub value: String,
}

impl Variable {
    pub fn new(name: String, value: String) -> Self {
        Self { name, value }
    }
}
