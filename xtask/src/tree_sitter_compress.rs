use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Deserialize, Serialize)]
struct NodeType {
    #[serde(rename = "type")]
    type_name: String,
    named: bool,
    #[serde(default)]
    subtypes: Option<Vec<NodeType>>,
    #[serde(default)]
    fields: Option<HashMap<String, FieldType>>,
    #[serde(default)]
    children: Option<FieldType>,
}

#[derive(Debug, Deserialize, Serialize)]
struct FieldType {
    multiple: bool,
    required: bool,
    types: Vec<TypeRef>,
}

#[derive(Debug, Deserialize, Serialize)]
struct TypeRef {
    #[serde(rename = "type")]
    type_name: String,
    named: bool,
}

#[derive(Debug)]
pub enum Lang {
    JavaScript,
    TypeScript,
    Tsx,
    Html,
    Css,
    Angular,
    Java,
    Kotlin,
    Scala,
    Python,
    Go,
    Rust,
    Json,
    CSharp,
    Cpp,
    C,
    Php,
    Ruby,
    Elixir,
    Yaml,
}

impl Lang {
    pub fn as_str(&self) -> &'static str {
        match self {
            Lang::JavaScript => "javascript",
            Lang::TypeScript => "typescript",
            Lang::Tsx => "tsx",
            Lang::Html => "html",
            Lang::Css => "css",
            Lang::Angular => "angular",
            Lang::Java => "java",
            Lang::Kotlin => "kotlin",
            Lang::Scala => "scala",
            Lang::Python => "python",
            Lang::Go => "go",
            Lang::Rust => "rust",
            Lang::Json => "json",
            Lang::CSharp => "c_sharp",
            Lang::Cpp => "cpp",
            Lang::C => "c",
            Lang::Php => "php",
            Lang::Ruby => "ruby",
            Lang::Elixir => "elixir",
            Lang::Yaml => "yaml",
        }
    }

    pub fn node_types_url(&self) -> &'static str {
        match self {
            Lang::JavaScript => "https://raw.githubusercontent.com/tree-sitter/tree-sitter-javascript/refs/tags/v0.23.0/src/node-types.json",
            Lang::TypeScript => "https://raw.githubusercontent.com/tree-sitter/tree-sitter-typescript/refs/tags/v0.23.2/typescript/src/node-types.json",
            Lang::Tsx => "https://raw.githubusercontent.com/tree-sitter/tree-sitter-typescript/refs/tags/v0.23.2/tsx/src/node-types.json",
            Lang::Html => "https://raw.githubusercontent.com/tree-sitter/tree-sitter-html/refs/tags/v0.23.0/src/node-types.json",
            Lang::Css => "https://raw.githubusercontent.com/tree-sitter/tree-sitter-css/refs/tags/v0.23.2/src/node-types.json",
            Lang::Angular => "https://raw.githubusercontent.com/codemod-com/tree-sitter-angular/refs/heads/main/src/node-types.json",
            Lang::Java => "https://raw.githubusercontent.com/tree-sitter/tree-sitter-java/refs/tags/v0.23.0/src/node-types.json",
            Lang::Kotlin => "https://raw.githubusercontent.com/fwcd/tree-sitter-kotlin/refs/tags/0.3.8/src/node-types.json",
            Lang::Scala => "https://raw.githubusercontent.com/tree-sitter/tree-sitter-scala/refs/tags/v0.23.0/src/node-types.json",
            Lang::Python => "https://raw.githubusercontent.com/tree-sitter/tree-sitter-python/refs/tags/v0.23.0/src/node-types.json",
            Lang::Go => "https://raw.githubusercontent.com/tree-sitter/tree-sitter-go/refs/tags/v0.23.0/src/node-types.json",
            Lang::Rust => "https://raw.githubusercontent.com/tree-sitter/tree-sitter-rust/refs/tags/v0.23.0/src/node-types.json",
            Lang::Json => "https://raw.githubusercontent.com/tree-sitter/tree-sitter-json/refs/tags/v0.23.0/src/node-types.json",
            Lang::CSharp => "https://raw.githubusercontent.com/tree-sitter/tree-sitter-c-sharp/refs/tags/v0.23.0/src/node-types.json",
            Lang::Cpp => "https://raw.githubusercontent.com/tree-sitter/tree-sitter-cpp/refs/tags/v0.23.0/src/node-types.json",
            Lang::C => "https://raw.githubusercontent.com/tree-sitter/tree-sitter-c/refs/tags/v0.23.0/src/node-types.json",
            Lang::Php => "https://github.com/tree-sitter/tree-sitter-php/raw/refs/heads/master/php_only/src/node-types.json",
            Lang::Ruby => "https://raw.githubusercontent.com/tree-sitter/tree-sitter-ruby/refs/tags/v0.23.0/src/node-types.json",
            Lang::Elixir => "https://raw.githubusercontent.com/elixir-lang/tree-sitter-elixir/refs/tags/v0.3.4/src/node-types.json",
            Lang::Yaml => "https://raw.githubusercontent.com/tree-sitter-grammars/tree-sitter-yaml/refs/heads/master/src/node-types.json",
        }
    }

    pub fn all() -> Vec<Lang> {
        vec![
            Lang::JavaScript,
            Lang::TypeScript,
            Lang::Tsx,
            Lang::Html,
            Lang::Css,
            Lang::Angular,
            Lang::Java,
            Lang::Kotlin,
            Lang::Scala,
            Lang::Python,
            Lang::Go,
            Lang::Rust,
            Lang::Json,
            Lang::CSharp,
            Lang::Cpp,
            Lang::C,
            Lang::Php,
            Lang::Ruby,
            Lang::Elixir,
            Lang::Yaml,
        ]
    }
}

fn filter_out_unnamed_node(mut node: NodeType) -> Option<NodeType> {
    if !node.named {
        return None;
    }

    if let Some(ref mut fields) = node.fields {
        for field in fields.values_mut() {
            field.types.retain(|t| t.named);
        }
    }

    if let Some(ref mut children) = node.children {
        children.types.retain(|t| t.named);
    }

    if let Some(ref mut subtypes) = node.subtypes {
        subtypes.retain(|n| n.named);
    }

    Some(node)
}

fn process_node_types(node_types: Vec<NodeType>) -> HashMap<String, String> {
    let mut node_definitions = HashMap::new();

    for node in node_types {
        if let Some(node) = filter_out_unnamed_node(node) {
            let mut fields = Vec::new();

            if let Some(node_fields) = node.fields {
                for (key, field) in node_fields {
                    let types: Vec<String> = field
                        .types
                        .iter()
                        .filter(|t| t.named)
                        .map(|t| t.type_name.clone())
                        .collect();

                    if !types.is_empty() {
                        let types_str = types.join(",");
                        let optional = if field.required { "" } else { "?" };
                        let multiple = if field.multiple { "*" } else { "" };
                        fields.push(format!("{key}={types_str}{multiple}{optional}"));
                    }
                }
            }

            if let Some(children) = node.children {
                let types: Vec<String> =
                    children.types.iter().map(|t| t.type_name.clone()).collect();

                if !types.is_empty() {
                    let types_str = types.join(", ");
                    let optional = if children.required { "" } else { "?" };
                    let multiple = if children.multiple { "*" } else { "" };
                    fields.push(format!("children={types_str}{multiple}{optional}"));
                }
            }

            if !fields.is_empty() {
                let definition = format!("{}: {}", node.type_name, fields.join(", "));
                node_definitions.insert(node.type_name, definition);
            }
        }
    }

    node_definitions
}

pub async fn compress_tree_sitter_grammar() -> Result<()> {
    let output_dir = Path::new("crates/mcp/src/data/node_types");
    fs::create_dir_all(output_dir)?;

    let client = reqwest::Client::new();

    for lang in Lang::all() {
        println!("Processing {}...", lang.as_str());

        let response = client.get(lang.node_types_url()).send().await?;
        let node_types_json = response.text().await?;
        let node_types: Vec<NodeType> = serde_json::from_str(&node_types_json)?;

        let node_definitions = process_node_types(node_types);
        let compressed_content = node_definitions
            .values()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");

        let output_path = output_dir.join(format!("{}.txt", lang.as_str()));
        fs::write(output_path, compressed_content)?;

        println!("✓ Generated compressed node types for {}", lang.as_str());
    }

    println!("✅ All tree-sitter node types compressed successfully!");
    Ok(())
}
