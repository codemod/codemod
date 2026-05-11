use butterflow_models::Error as ModelError;
use miette::{Diagnostic, GraphicalReportHandler, GraphicalTheme, NamedSource, SourceSpan};
use std::fmt::Write as _;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error, Diagnostic)]
#[error("{message}")]
#[diagnostic(code(codemod::cli::error))]
struct CliErrorDiagnostic {
    message: String,
    #[help]
    help: Option<String>,
}

#[derive(Debug, Error, Diagnostic)]
#[error("{message}")]
#[diagnostic(code(codemod::runtime::javascript))]
struct JavaScriptRuntimeDiagnostic {
    message: String,
    #[source_code]
    source_code: NamedSource<String>,
    #[label("error originated here")]
    span: SourceSpan,
    #[help]
    help: Option<String>,
}

#[derive(Debug, Error, Diagnostic)]
#[error("{message}")]
#[diagnostic(code(codemod::runtime::ast_grep))]
struct AstGrepDiagnostic {
    message: String,
    #[help]
    help: Option<String>,
}

#[derive(Debug, Error, Diagnostic)]
#[error("{message}")]
#[diagnostic(code(codemod::workflow::config))]
struct WorkflowConfigDiagnostic {
    message: String,
    #[help]
    help: Option<String>,
}

#[derive(Debug, Error, Diagnostic)]
#[error("{message}")]
#[diagnostic(code(codemod::workflow::config))]
struct WorkflowConfigSourceDiagnostic {
    message: String,
    #[source_code]
    source_code: NamedSource<String>,
    #[label("invalid workflow field")]
    span: SourceSpan,
    #[help]
    help: Option<String>,
}

#[derive(Debug, Error, Diagnostic)]
#[error("{message}")]
#[diagnostic(code(codemod::runtime::shell))]
struct ShellCommandDiagnostic {
    message: String,
    #[source_code]
    source_code: NamedSource<String>,
    #[label("command failed here")]
    span: SourceSpan,
    #[help]
    help: Option<String>,
}

#[derive(Debug, Error)]
#[error("Shell command failed with exit code {exit_code}")]
pub(crate) struct ShellCommandFailure {
    pub(crate) command: String,
    pub(crate) exit_code: i32,
    pub(crate) output: String,
}

#[derive(Debug, Error)]
#[error("{message}")]
pub(crate) struct AstGrepFailure {
    pub(crate) message: String,
    pub(crate) help: Option<String>,
}

#[derive(Debug)]
struct JsStackLocation {
    path: String,
    line: usize,
    column: usize,
}

#[derive(Debug)]
struct LineColumnLocation {
    line: usize,
    column: usize,
}

#[derive(Debug)]
struct WorkflowParseDetails {
    path: PathBuf,
    yaml_error: String,
    yaml_line: Option<usize>,
    yaml_column: Option<usize>,
    json_error: String,
}

pub(crate) fn render_anyhow_error(error: &anyhow::Error) {
    let rendered = render_error_text(error);
    eprintln!("{rendered}");
}

pub(crate) fn render_error_message(message: &str) -> String {
    render_error_text(&anyhow::anyhow!("{}", message))
}

fn render_error_text(error: &anyhow::Error) -> String {
    if let Some(rendered) = render_javascript_runtime_error(error) {
        return rendered;
    }
    if let Some(rendered) = render_workflow_config_error(error) {
        return rendered;
    }
    if let Some(rendered) = render_ast_grep_error(error) {
        return rendered;
    }
    if let Some(rendered) = render_shell_command_error(error) {
        return rendered;
    }

    let diagnostic = CliErrorDiagnostic {
        message: error.to_string(),
        help: error_chain_help(error),
    };
    render_report(&diagnostic)
}

fn render_javascript_runtime_error(error: &anyhow::Error) -> Option<String> {
    let error_text = format!("{error:#}");
    let location = parse_js_stack_location(&error_text)?;
    let source = std::fs::read_to_string(&location.path).ok()?;
    let offset = line_column_to_byte_offset(&source, location.line, location.column)?;
    let message = first_error_line(&error_text).unwrap_or_else(|| error.to_string());
    let diagnostic = JavaScriptRuntimeDiagnostic {
        message,
        source_code: NamedSource::new(location.path, source),
        span: (offset, 1usize).into(),
        help: Some("Fix the thrown JavaScript/TypeScript error and rerun the codemod.".to_string()),
    };
    Some(render_report(&diagnostic))
}

fn render_workflow_config_error(error: &anyhow::Error) -> Option<String> {
    if let Some(parse_error) = workflow_parse_error_from_chain(error) {
        let message = workflow_parse_message(&parse_error);
        if let Some(diagnostic) = workflow_parse_source_diagnostic(&parse_error, &message) {
            return Some(render_report(&diagnostic));
        }

        let diagnostic = WorkflowConfigDiagnostic {
            help: workflow_config_help(&message),
            message,
        };
        return Some(render_report(&diagnostic));
    }

    let error_text = format!("{error:#}");
    let message = workflow_config_error_message(&error_text)?;
    if let Some(diagnostic) = workflow_config_source_diagnostic(&error_text, &message) {
        return Some(render_report(&diagnostic));
    }

    let diagnostic = WorkflowConfigDiagnostic {
        help: workflow_config_help(&message),
        message,
    };
    Some(render_report(&diagnostic))
}

fn workflow_parse_error_from_chain(error: &anyhow::Error) -> Option<WorkflowParseDetails> {
    error.chain().find_map(|cause| {
        let ModelError::WorkflowParse {
            path,
            yaml_error,
            yaml_line,
            yaml_column,
            json_error,
            ..
        } = cause.downcast_ref::<ModelError>()?
        else {
            return None;
        };

        Some(WorkflowParseDetails {
            path: path.clone(),
            yaml_error: yaml_error.to_string(),
            yaml_line: *yaml_line,
            yaml_column: *yaml_column,
            json_error: json_error.to_string(),
        })
    })
}

fn workflow_parse_message(parse_error: &WorkflowParseDetails) -> String {
    format!(
        "YAML error: {}, JSON error: {}",
        parse_error.yaml_error, parse_error.json_error
    )
}

fn workflow_parse_source_diagnostic(
    parse_error: &WorkflowParseDetails,
    message: &str,
) -> Option<WorkflowConfigSourceDiagnostic> {
    let source = std::fs::read_to_string(&parse_error.path).ok()?;
    let line = parse_error.yaml_line?;
    let column = parse_error.yaml_column?;
    let offset = line_column_to_byte_offset(&source, line, column)?;
    let span_len = token_span_len_at(&source, offset).unwrap_or(1);

    Some(WorkflowConfigSourceDiagnostic {
        message: message.to_string(),
        source_code: NamedSource::new(parse_error.path.display().to_string(), source),
        span: (offset, span_len).into(),
        help: workflow_config_help(message),
    })
}

fn workflow_config_source_diagnostic(
    error_text: &str,
    message: &str,
) -> Option<WorkflowConfigSourceDiagnostic> {
    let path = workflow_file_path_from_error(error_text)?;
    let source = std::fs::read_to_string(&path).ok()?;
    let location = yaml_error_location(message)?;
    let offset = line_column_to_byte_offset(&source, location.line, location.column)?;
    let span_len = token_span_len_at(&source, offset).unwrap_or(1);

    Some(WorkflowConfigSourceDiagnostic {
        message: message.to_string(),
        source_code: NamedSource::new(path, source),
        span: (offset, span_len).into(),
        help: workflow_config_help(message),
    })
}

fn render_ast_grep_error(error: &anyhow::Error) -> Option<String> {
    if let Some(failure) = error.downcast_ref::<AstGrepFailure>() {
        let diagnostic = AstGrepDiagnostic {
            help: failure.help.clone(),
            message: failure.message.clone(),
        };
        return Some(render_report(&diagnostic));
    }

    let error_text = format!("{error:#}");
    let message = first_ast_grep_error_line(&error_text)?;
    let diagnostic = AstGrepDiagnostic {
        help: ast_grep_help(&message),
        message,
    };
    Some(render_report(&diagnostic))
}

fn render_shell_command_error(error: &anyhow::Error) -> Option<String> {
    if let Some(failure) = error.downcast_ref::<ShellCommandFailure>() {
        return Some(render_shell_command_failure(
            &failure.command,
            failure.exit_code,
            Some(&failure.output),
        ));
    }

    let error_text = format!("{error:#}");
    let start = error_text.find("Shell command failed")?;
    let failure = &error_text[start..];
    let (message, body) = failure.split_once("\n\nCommand:\n")?;
    let (command, output) = body
        .split_once("\n\nOutput:\n")
        .map_or((body, None), |(command, output)| (command, Some(output)));
    let command = command.trim_end().to_string();
    if command.is_empty() {
        return None;
    }

    Some(render_shell_command_failure(
        &command,
        parse_shell_exit_code(message).unwrap_or(1),
        output,
    ))
}

fn render_shell_command_failure(command: &str, exit_code: i32, output: Option<&str>) -> String {
    let help = output
        .map(str::trim)
        .filter(|output| !output.is_empty())
        .map(|output| format!("Process output:\n{}", truncate_help_output(output)));
    let diagnostic = ShellCommandDiagnostic {
        message: format!("Shell command failed with exit code {exit_code}"),
        source_code: NamedSource::new("shell command", command.to_string()),
        span: (0usize, command.len()).into(),
        help,
    };
    render_report(&diagnostic)
}

fn parse_shell_exit_code(message: &str) -> Option<i32> {
    message
        .strip_prefix("Shell command failed with exit code ")?
        .trim()
        .parse()
        .ok()
}

fn workflow_config_help(message: &str) -> Option<String> {
    if let Some(help) = invalid_type_help(message) {
        return Some(help);
    }

    Some("Check the workflow YAML shape and field types, then rerun the workflow.".to_string())
}

fn ast_grep_help(message: &str) -> Option<String> {
    if message.starts_with("Config error:") || message.contains("rule configuration") {
        Some(
            "Check the ast-grep YAML rule syntax, pattern language, and fixer configuration."
                .to_string(),
        )
    } else if message.starts_with("Language error:") {
        Some(
            "Check that the target file extension maps to a supported ast-grep language."
                .to_string(),
        )
    } else if message.starts_with("IO error:") {
        Some("Check that the target file exists and is readable, and that modified files are writable.".to_string())
    } else {
        Some("Fix the ast-grep rule or target file issue and rerun the workflow.".to_string())
    }
}

fn truncate_help_output(output: &str) -> String {
    const MAX_CHARS: usize = 2000;
    if output.len() <= MAX_CHARS {
        return output.to_string();
    }

    let mut truncated = output
        .char_indices()
        .take_while(|(index, _)| *index < MAX_CHARS)
        .map(|(_, ch)| ch)
        .collect::<String>();
    truncated.push_str("\n... output truncated ...");
    truncated
}

fn render_report(diagnostic: &(dyn Diagnostic + Send + Sync + 'static)) -> String {
    let mut rendered = String::new();
    let handler = GraphicalReportHandler::new_themed(GraphicalTheme::unicode());
    if handler.render_report(&mut rendered, diagnostic).is_ok() {
        rendered
    } else {
        diagnostic.to_string()
    }
}

fn error_chain_help(error: &anyhow::Error) -> Option<String> {
    let causes = error
        .chain()
        .skip(1)
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if causes.is_empty() {
        return None;
    }

    let mut help = String::from("Caused by:");
    for cause in causes {
        let _ = write!(help, "\n  - {cause}");
    }
    Some(help)
}

fn first_ast_grep_error_line(error_text: &str) -> Option<String> {
    const AST_GREP_ERROR_MARKERS: &[&str] = &[
        "Config error:",
        "Language error:",
        "IO error:",
        "Path error:",
        "Glob error:",
        "YAML error:",
        "JSON error:",
        "AST grep config file not found:",
        "AST-grep rule configuration error",
    ];

    error_text.lines().map(str::trim).find_map(|line| {
        AST_GREP_ERROR_MARKERS
            .iter()
            .filter_map(|marker| line.find(marker).map(|index| line[index..].to_string()))
            .next()
    })
}

fn workflow_config_error_message(error_text: &str) -> Option<String> {
    if !is_workflow_config_error(error_text) {
        return None;
    }

    error_text.lines().map(str::trim).find_map(|line| {
        line.find("YAML error:")
            .or_else(|| line.find("Workflow validation error:"))
            .map(|index| line[index..].to_string())
    })
}

fn is_workflow_config_error(error_text: &str) -> bool {
    error_text.contains("Failed to parse workflow file")
        || error_text.contains("Failed to validate workflow file")
        || error_text.contains("Failed to validate workflow")
}

fn invalid_type_help(message: &str) -> Option<String> {
    let (field, rest) = message.split_once(": invalid type: ")?;
    let field = field
        .strip_prefix("YAML error: ")
        .or_else(|| field.strip_prefix("JSON error: "))
        .unwrap_or(field)
        .trim();
    if field.is_empty() {
        return None;
    }

    let expected = rest
        .split_once(", expected ")
        .map(|(_, expected)| expected)
        .and_then(|expected| expected.split(" at line ").next())
        .map(str::trim)
        .filter(|expected| !expected.is_empty())?;
    let actual = rest
        .split_once(", expected ")
        .map(|(actual, _)| actual.trim())
        .filter(|actual| !actual.is_empty())?;

    Some(format!(
        "Check `{field}` in the workflow YAML: expected {expected}, but got {actual}."
    ))
}

fn workflow_file_path_from_error(error_text: &str) -> Option<String> {
    error_text.lines().map(str::trim).find_map(|line| {
        let path = line.strip_prefix("Failed to parse workflow file: ")?;
        let path = path
            .split_once(": Workflow validation error:")
            .map(|(path, _)| path)
            .unwrap_or(path)
            .trim();
        if path.is_empty() {
            None
        } else {
            Some(path.to_string())
        }
    })
}

fn yaml_error_location(message: &str) -> Option<LineColumnLocation> {
    let yaml_message = message
        .split_once(", JSON error:")
        .map(|(yaml, _)| yaml)
        .unwrap_or(message);
    let (_, rest) = yaml_message.rsplit_once(" at line ")?;
    let (line, rest) = rest.split_once(" column ")?;
    let column = rest
        .split(|ch: char| !ch.is_ascii_digit())
        .next()
        .unwrap_or_default();
    Some(LineColumnLocation {
        line: line.parse().ok()?,
        column: column.parse().ok()?,
    })
}

fn token_span_len_at(source: &str, offset: usize) -> Option<usize> {
    let tail = source.get(offset..)?;
    let len = tail
        .char_indices()
        .take_while(|(_, ch)| !ch.is_whitespace() && !matches!(ch, ',' | '}' | ']'))
        .last()
        .map(|(index, ch)| index + ch.len_utf8())?;
    Some(len.max(1))
}

fn first_error_line(error_text: &str) -> Option<String> {
    const JS_ERROR_MARKERS: &[&str] = &[
        "Error:",
        "TypeError:",
        "ReferenceError:",
        "SyntaxError:",
        "RangeError:",
    ];

    error_text.lines().map(str::trim).find_map(|line| {
        JS_ERROR_MARKERS
            .iter()
            .filter_map(|marker| line.find(marker).map(|index| line[index..].to_string()))
            .next()
    })
}

fn parse_js_stack_location(error_text: &str) -> Option<JsStackLocation> {
    for line in error_text.lines() {
        let line = line.trim();
        let Some(start) = line.rfind('(') else {
            continue;
        };
        let Some(end) = line.rfind(')') else {
            continue;
        };
        if end <= start {
            continue;
        }
        let location = &line[start + 1..end];
        if let Some(parsed) = parse_js_location(location) {
            return Some(parsed);
        }
    }
    None
}

fn parse_js_location(location: &str) -> Option<JsStackLocation> {
    let (path_and_line, column) = location.rsplit_once(':')?;
    let (path, line) = path_and_line.rsplit_once(':')?;
    let line = line.parse::<usize>().ok()?;
    let column = column.parse::<usize>().ok()?;
    if path.is_empty() || line == 0 || column == 0 {
        return None;
    }
    Some(JsStackLocation {
        path: path.to_string(),
        line,
        column,
    })
}

fn line_column_to_byte_offset(source: &str, line: usize, column: usize) -> Option<usize> {
    let mut offset = 0usize;
    for (index, source_line) in source.split_inclusive('\n').enumerate() {
        if index + 1 == line {
            let line_without_newline = source_line.trim_end_matches(['\r', '\n']);
            let column_offset = line_without_newline
                .char_indices()
                .nth(column.saturating_sub(1))
                .map(|(idx, _)| idx)
                .unwrap_or(line_without_newline.len());
            return Some(offset + column_offset);
        }
        offset += source_line.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_js_stack_location() {
        let location = parse_js_stack_location(
            "Error: Test error\n    at codemod (/tmp/scripts/codemod.ts:16:13)\n",
        )
        .expect("location");

        assert_eq!(location.path, "/tmp/scripts/codemod.ts");
        assert_eq!(location.line, 16);
        assert_eq!(location.column, 13);
    }

    #[test]
    fn maps_line_column_to_byte_offset() {
        let source = "one\nthrow new Error('x')\n";
        let offset = line_column_to_byte_offset(source, 2, 7).expect("offset");
        assert_eq!(&source[offset..offset + 3], "new");
    }

    #[test]
    fn extracts_js_error_line_from_wrapped_runtime_error() {
        let message = first_error_line(
            "Runtime error JavaScript execution failed: Error: Test error\n    at codemod (/tmp/codemod.ts:16:13)",
        )
        .expect("error line");

        assert_eq!(message, "Error: Test error");
    }

    #[test]
    fn extracts_ast_grep_error_line_from_wrapped_step_error() {
        let message = first_ast_grep_error_line(
            "Step ast-grep failed: Failed to process input.ts: Config error: invalid pattern",
        )
        .expect("error line");

        assert_eq!(message, "Config error: invalid pattern");
    }

    #[test]
    fn workflow_yaml_parse_error_is_not_rendered_as_ast_grep() {
        let rendered = render_error_message(
            "Failed to parse workflow file. YAML error: nodes[0].trigger: invalid type: string \"manual\", expected struct Trigger at line 8 column 14, JSON error: expected value at line 1 column 1",
        );

        assert!(rendered.contains("codemod::workflow::config"));
        assert!(rendered.contains("nodes[0].trigger"));
        assert!(rendered.contains(
            "Check `nodes[0].trigger` in the workflow YAML: expected struct Trigger, but got string \"manual\"."
        ));
        assert!(!rendered.contains("codemod::runtime::ast_grep"));
        assert!(!rendered.contains("Fix the ast-grep rule"));
    }

    #[test]
    fn workflow_yaml_parse_error_uses_source_span_when_path_is_available() {
        let file = tempfile::NamedTempFile::new().expect("temp workflow");
        std::fs::write(
            file.path(),
            "nodes:\n  - id: apply-transforms\n    name: Apply AST Transformations\n    trigger: manual\n    type: automatic\n",
        )
        .expect("write workflow");
        let rendered = render_error_message(&format!(
            "Failed to parse workflow file: {}\nFailed to parse workflow file. YAML error: nodes[0].trigger: invalid type: string \"manual\", expected struct Trigger at line 4 column 14, JSON error: expected value at line 1 column 1",
            file.path().display()
        ));

        assert!(rendered.contains("codemod::workflow::config"));
        assert!(rendered.contains("manual"));
        assert!(rendered.contains("invalid workflow field"));
        assert!(rendered.contains(
            "Check `nodes[0].trigger` in the workflow YAML: expected struct Trigger, but got string \"manual\"."
        ));
    }

    #[test]
    fn structured_workflow_parse_error_uses_source_span() {
        let file = tempfile::NamedTempFile::new().expect("temp workflow");
        std::fs::write(
            file.path(),
            "nodes:\n  - id: apply-transforms\n    name: Apply AST Transformations\n    trigger: manual\n    type: automatic\n",
        )
        .expect("write workflow");
        let error = anyhow::anyhow!(ModelError::WorkflowParse {
            path: file.path().to_path_buf(),
            yaml_error: "nodes[0].trigger: invalid type: string \"manual\", expected struct Trigger at line 4 column 14".into(),
            yaml_line: Some(4),
            yaml_column: Some(14),
            json_error: "expected value at line 1 column 1".into(),
            json_line: Some(1),
            json_column: Some(1),
        });
        let rendered = render_error_text(&error);

        assert!(rendered.contains("codemod::workflow::config"));
        assert!(rendered.contains(":4:14"));
        assert!(rendered.contains("manual"));
        assert!(rendered.contains("invalid workflow field"));
    }

    #[test]
    fn renders_shell_command_failure_with_command_source() {
        let rendered = render_error_message(
            "Shell command failed with exit code 1\n\nCommand:\necho 'hello'\nfalse\n\nOutput:\nhello",
        );

        assert!(rendered.contains("codemod::runtime::shell"));
        assert!(rendered.contains("Shell command failed with exit code 1"));
        assert!(rendered.contains("echo 'hello'"));
        assert!(rendered.contains("false"));
        assert!(rendered.contains("command failed here"));
        assert!(rendered.contains("Process output:"));
        assert!(rendered.contains("hello"));
    }
}
