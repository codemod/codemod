use std::path::PathBuf;

use thiserror::Error;

use crate::RuntimeType;

#[derive(Error, Debug)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("YAML parsing error: {0}")]
    YamlParsing(#[from] serde_yaml::Error),

    #[error("JSON parsing error: {0}")]
    JsonParsing(#[from] serde_json::Error),

    #[error("Workflow validation error: {0}")]
    WorkflowValidation(String),

    #[error(
        "Failed to parse workflow file: {path}. YAML error: {yaml_error}, JSON error: {json_error}"
    )]
    WorkflowParse {
        path: PathBuf,
        yaml_error: Box<str>,
        yaml_line: Option<usize>,
        yaml_column: Option<usize>,
        json_error: Box<str>,
        json_line: Option<usize>,
        json_column: Option<usize>,
    },

    #[error("Node not found: {0}")]
    NodeNotFound(String),

    #[error("Task not found: {0}")]
    TaskNotFound(String),

    #[error("Cyclic dependency detected: {0}")]
    CyclicDependency(String),

    #[error("Variable resolution error: {0}")]
    VariableResolution(String),

    #[error("Expression evaluation error: {0}")]
    ExpressionEvaluation(#[from] evalexpr::EvalexprError),

    #[error("Invalid command. Expected string, got unknown type")]
    InvalidCommand,

    #[error("Runtime error: {0}")]
    Runtime(String),

    #[error("Command failed with exit code {exit_code}: {output}")]
    ShellCommandFailed { exit_code: i32, output: String },

    #[error("Shell command failed with exit code {exit_code}")]
    ShellCommandStepFailed {
        command: String,
        exit_code: i32,
        output: String,
    },

    #[error("{message}")]
    AstGrepStepFailed {
        message: String,
        help: Option<String>,
    },

    #[error("State error: {0}")]
    State(String),

    #[error("Template error: {0}")]
    Template(String),

    #[error("Matrix error: {0}")]
    Matrix(String),

    #[error("Docker error: {0}")]
    Docker(String),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Step execution error: {0}")]
    StepExecution(String),

    #[error("Deferred interaction: {0}")]
    Deferred(String),

    #[error("Other error: {0}")]
    Other(String),

    #[error("Unsupported runtime: {0}")]
    UnsupportedRuntime(RuntimeType),
}
