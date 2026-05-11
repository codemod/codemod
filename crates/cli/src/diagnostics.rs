use miette::{Diagnostic, GraphicalReportHandler, GraphicalTheme, NamedSource, SourceSpan};
use std::fmt::Write as _;
use thiserror::Error;

#[derive(Debug, Error)]
#[error("{message}")]
pub(crate) struct SilentExit {
    message: String,
}

impl SilentExit {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

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

#[derive(Debug)]
struct JsStackLocation {
    path: String,
    line: usize,
    column: usize,
}

pub(crate) fn render_anyhow_error(error: &anyhow::Error) {
    if error.downcast_ref::<SilentExit>().is_some() {
        return;
    }

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
    if let Some(rendered) = render_ast_grep_error(error) {
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

fn render_ast_grep_error(error: &anyhow::Error) -> Option<String> {
    let error_text = format!("{error:#}");
    let message = first_ast_grep_error_line(&error_text)?;
    let diagnostic = AstGrepDiagnostic {
        help: ast_grep_help(&message),
        message,
    };
    Some(render_report(&diagnostic))
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
}
