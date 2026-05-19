use anyhow::{anyhow, Context, Result};
use clap::Args;
use codemod_mcp::{CliResourceInfo, CliToolInfo, CodemodMcpServer};
use serde::Serialize;
use serde_json::{json, Value};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

#[derive(Args, Debug)]
pub struct DumpAstCommand {
    /// Source file to inspect. Omit or pass `-` to read from stdin.
    #[arg(value_name = "FILE")]
    source: Option<PathBuf>,
    /// Language to parse as. Defaults to file extension inference or `tsx` for stdin.
    #[arg(short, long)]
    language: Option<String>,
    /// Emit a JSON object with language and AST text.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
pub struct NodeTypesCommand {
    /// Language to inspect, such as tsx, typescript, javascript, python, or rust.
    language: String,
    /// Emit a JSON object with language and node type text.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
pub struct DocsCommand {
    /// Resource name, URI, or search query. Omit to list docs resources.
    #[arg(value_name = "DOCS", num_args = 0..)]
    query: Vec<String>,
    /// Emit JSON output.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
pub struct ToolsCommand {
    /// Emit JSON output.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
pub struct ToolCommand {
    /// Tool name.
    name: String,
    /// Emit JSON output.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
pub struct CallCommand {
    /// Tool name.
    name: String,
    /// JSON input, `@path/to/request.json`, or `-` for stdin. Defaults to `{}`.
    #[arg(long)]
    input: Option<String>,
}

#[derive(Args, Debug)]
pub struct ResourcesCommand {
    /// Emit JSON output.
    #[arg(long)]
    json: bool,
}

#[derive(Args, Debug)]
pub struct ResourceCommand {
    /// Resource URI or resource name.
    uri: String,
    /// Emit a JSON object with resource metadata and content.
    #[arg(long)]
    json: bool,
}

#[derive(Serialize)]
struct TextOutput<'a> {
    language: &'a str,
    text: &'a str,
}

#[derive(Serialize)]
struct ResourceContentOutput<'a> {
    resource: &'a CliResourceInfo,
    content: &'a str,
}

#[derive(Serialize)]
struct DocsSearchOutput<'a> {
    query: &'a str,
    matches: Vec<DocsSearchMatch>,
}

#[derive(Serialize)]
struct DocsSearchMatch {
    name: String,
    uri: String,
    line: usize,
    snippet: String,
}

pub async fn handle_dump_ast(command: &DumpAstCommand) -> Result<()> {
    let source_code = read_source(command.source.as_deref())?;
    let language = command
        .language
        .clone()
        .or_else(|| command.source.as_deref().and_then(infer_language_from_path))
        .unwrap_or_else(|| "tsx".to_string());
    let server = CodemodMcpServer::default();
    let ast = server.dump_ast_text(&source_code, &language).await?;

    if command.json {
        print_json(&TextOutput {
            language: &language,
            text: &ast,
        })
    } else {
        println!("{ast}");
        Ok(())
    }
}

pub async fn handle_node_types(command: &NodeTypesCommand) -> Result<()> {
    let server = CodemodMcpServer::default();
    let node_types = server.node_types_text(&command.language).await?;

    if command.json {
        print_json(&TextOutput {
            language: &command.language,
            text: &node_types,
        })
    } else {
        println!("{node_types}");
        Ok(())
    }
}

pub async fn handle_docs(command: &DocsCommand) -> Result<()> {
    let server = CodemodMcpServer::default();
    let resources = server.cli_resources();
    let query = command.query.join(" ");
    let query = query.trim();

    if query.is_empty() {
        return print_resources(&resources, command.json);
    }

    if let Some(resource) = resolve_resource(&resources, query) {
        let content = server.read_resource_text_live(resource.uri).await?;
        if command.json {
            return print_json(&ResourceContentOutput {
                resource,
                content: &content,
            });
        }
        println!("{content}");
        return Ok(());
    }

    let matches = search_docs(&server, &resources, query).await?;
    if command.json {
        return print_json(&DocsSearchOutput { query, matches });
    }

    if matches.is_empty() {
        println!("No docs matched '{query}'.");
    } else {
        for matched in matches {
            println!("{} ({})", matched.name, matched.uri);
            println!("  line {}: {}", matched.line, matched.snippet);
        }
    }
    Ok(())
}

pub fn handle_tools(command: &ToolsCommand) -> Result<()> {
    let server = CodemodMcpServer::default();
    let tools = server.cli_tools();
    if command.json {
        print_json(&tools)
    } else {
        for tool in tools {
            println!("{}\t{}", tool.name, tool.description);
        }
        Ok(())
    }
}

pub fn handle_tool(command: &ToolCommand) -> Result<()> {
    let server = CodemodMcpServer::default();
    let tools = server.cli_tools();
    let tool = resolve_tool(&tools, &command.name)
        .ok_or_else(|| anyhow!("Unknown MCP tool '{}'", command.name))?;

    if command.json {
        print_json(tool)
    } else {
        println!("{}", tool.name);
        println!("{}", tool.description);
        println!();
        println!("{}", serde_json::to_string_pretty(&tool.input_schema)?);
        Ok(())
    }
}

pub async fn handle_call(command: &CallCommand) -> Result<()> {
    let arguments = read_json_input(command.input.as_deref())?;
    let server = CodemodMcpServer::default();
    let output = server.call_tool_text(&command.name, arguments).await?;
    println!("{output}");
    Ok(())
}

pub fn handle_resources(command: &ResourcesCommand) -> Result<()> {
    let server = CodemodMcpServer::default();
    let resources = server.cli_resources();
    print_resources(&resources, command.json)
}

pub async fn handle_resource(command: &ResourceCommand) -> Result<()> {
    let server = CodemodMcpServer::default();
    let resources = server.cli_resources();
    let resource = resolve_resource(&resources, &command.uri)
        .ok_or_else(|| anyhow!("Unknown MCP resource '{}'", command.uri))?;
    let content = server.read_resource_text_live(resource.uri).await?;

    if command.json {
        print_json(&ResourceContentOutput {
            resource,
            content: &content,
        })
    } else {
        println!("{content}");
        Ok(())
    }
}

fn read_source(source: Option<&Path>) -> Result<String> {
    match source {
        Some(path) if path != Path::new("-") => fs::read_to_string(path)
            .with_context(|| format!("Failed to read source file '{}'", path.display())),
        _ => read_stdin(),
    }
}

fn read_stdin() -> Result<String> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .context("Failed to read stdin")?;
    Ok(input)
}

fn read_json_input(input: Option<&str>) -> Result<Value> {
    let raw = match input {
        None => "{}".to_string(),
        Some("-") => read_stdin()?,
        Some(path) if path.starts_with('@') => {
            let path = Path::new(path.trim_start_matches('@'));
            fs::read_to_string(path)
                .with_context(|| format!("Failed to read JSON input file '{}'", path.display()))?
        }
        Some(inline) => inline.to_string(),
    };

    if raw.trim().is_empty() {
        return Ok(json!({}));
    }

    serde_json::from_str(&raw).context("Failed to parse JSON input")
}

fn infer_language_from_path(path: &Path) -> Option<String> {
    let extension = path.extension()?.to_string_lossy().to_lowercase();
    let language = match extension.as_str() {
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "tsx",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "py" | "pyw" | "pyi" => "python",
        "rs" => "rust",
        "java" => "java",
        "go" => "go",
        "cpp" | "cxx" | "cc" | "c++" | "hpp" | "hxx" | "hh" | "h++" => "cpp",
        "c" | "h" => "c",
        "cs" => "csharp",
        "html" | "htm" => "html",
        "xml" | "csproj" | "props" | "targets" | "config" | "resx" | "xaml" => "xml",
        "css" => "css",
        "json" | "jsonc" => "json",
        "yaml" | "yml" => "yaml",
        "php" | "phtml" | "php3" | "php4" | "php5" | "php7" | "phps" => "php",
        "rb" | "rbw" => "ruby",
        "kt" | "kts" => "kotlin",
        "scala" | "sc" => "scala",
        "ex" | "exs" => "elixir",
        _ => return None,
    };
    Some(language.to_string())
}

fn resolve_tool<'a>(tools: &'a [CliToolInfo], name: &str) -> Option<&'a CliToolInfo> {
    let normalized = normalize_tool_lookup_key(name);
    tools
        .iter()
        .find(|tool| normalize_tool_lookup_key(tool.name) == normalized)
}

fn resolve_resource<'a>(
    resources: &'a [CliResourceInfo],
    query: &str,
) -> Option<&'a CliResourceInfo> {
    let normalized = query.to_lowercase();

    resources
        .iter()
        .find(|resource| {
            resource.uri.eq_ignore_ascii_case(query)
                || resource.name.eq_ignore_ascii_case(query)
                || resource
                    .name
                    .strip_suffix("-instructions")
                    .is_some_and(|name| name.eq_ignore_ascii_case(query))
                || resource
                    .uri
                    .split_once("://")
                    .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case(query))
        })
        .or_else(|| {
            resources.iter().find(|resource| {
                resource.name.to_lowercase().contains(&normalized)
                    || resource.uri.to_lowercase().contains(&normalized)
            })
        })
}

async fn search_docs(
    server: &CodemodMcpServer,
    resources: &[CliResourceInfo],
    query: &str,
) -> Result<Vec<DocsSearchMatch>> {
    let query_lower = query.to_lowercase();
    let mut matches = Vec::new();

    for resource in resources {
        if resource.name.to_lowercase().contains(&query_lower)
            || resource
                .description
                .unwrap_or_default()
                .to_lowercase()
                .contains(&query_lower)
        {
            matches.push(DocsSearchMatch {
                name: resource.name.to_string(),
                uri: resource.uri.to_string(),
                line: 0,
                snippet: resource.description.unwrap_or_default().to_string(),
            });
            continue;
        }

        let content = server.read_resource_text_cached(resource.uri).await?;
        if let Some((index, line)) = content
            .lines()
            .enumerate()
            .find(|(_, line)| line.to_lowercase().contains(&query_lower))
        {
            matches.push(DocsSearchMatch {
                name: resource.name.to_string(),
                uri: resource.uri.to_string(),
                line: index + 1,
                snippet: line.trim().to_string(),
            });
        }
    }

    Ok(matches)
}

fn normalize_lookup_key(value: &str) -> String {
    value.replace('-', "_").to_lowercase()
}

fn normalize_tool_lookup_key(value: &str) -> String {
    match normalize_lookup_key(value).as_str() {
        "node_types" => "get_node_types".to_string(),
        "dump_ast" => "dump_ast".to_string(),
        normalized => normalized.to_string(),
    }
}

fn print_resources(resources: &[CliResourceInfo], json: bool) -> Result<()> {
    if json {
        print_json(resources)
    } else {
        for resource in resources {
            println!(
                "{}\t{}\t{}",
                resource.name,
                resource.uri,
                resource.description.unwrap_or_default()
            );
        }
        Ok(())
    }
}

fn print_json<T: Serialize + ?Sized>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
