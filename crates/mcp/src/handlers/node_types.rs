use rmcp::{handler::server::wrapper::Parameters, model::*, schemars, tool, ErrorData as McpError};

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetNodeTypesRequest {
    /// The programming language to get node types for (e.g., "javascript", "typescript", "python", "rust", etc.)
    pub language: String,
}

#[derive(Clone)]
pub struct NodeTypesHandler;

impl NodeTypesHandler {
    pub fn new() -> Self {
        Self
    }

    #[tool(
        description = "Get tree-sitter node types for a specific programming language in AI-friendly format. You should use this tool to get the node types for the language you are working in. You do not know the node types for the language you are working in, so you should use this tool to get them."
    )]
    pub async fn get_node_types(
        &self,
        Parameters(request): Parameters<GetNodeTypesRequest>,
    ) -> Result<CallToolResult, McpError> {
        let node_types = match self.get_node_types_for_language(&request.language) {
            Some(types) => types,
            None => {
                return Err(McpError::invalid_params(
                    format!("Unsupported language '{}'", request.language),
                    None,
                ));
            }
        };

        let result = format!(
            r#"<TREE_SITTER_NODE_TYPES>
{node_types}
</TREE_SITTER_NODE_TYPES>

<LEGEND>
Legends for field notation:
- \`?\` - optional field (may not be present in all instances)
- \`*\` - multiple values allowed (array/list of values)

In tree-sitter grammar:
- Fields are named children with specific roles in the syntax tree
- Format: \`fieldName=nodeType\` (e.g., "body=block")
- When a field is not named, it's represented as \`children=nodeType\`
- Multiple possible types are comma-separated (e.g., "value=string,number")
</LEGEND>
"#
        );

        Ok(CallToolResult::success(vec![Content::text(result)]))
    }

    fn get_node_types_for_language(&self, language: &str) -> Option<&'static str> {
        match language.to_lowercase().as_str() {
            "javascript" | "js" => Some(include_str!("../data/node_types/javascript.txt")),
            "typescript" | "ts" => Some(include_str!("../data/node_types/typescript.txt")),
            "tsx" => Some(include_str!("../data/node_types/tsx.txt")),
            "python" | "py" => Some(include_str!("../data/node_types/python.txt")),
            "rust" | "rs" => Some(include_str!("../data/node_types/rust.txt")),
            "java" => Some(include_str!("../data/node_types/java.txt")),
            "go" => Some(include_str!("../data/node_types/go.txt")),
            "cpp" | "c++" | "cxx" => Some(include_str!("../data/node_types/cpp.txt")),
            "c" => Some(include_str!("../data/node_types/c.txt")),
            "csharp" | "c#" | "c_sharp" => Some(include_str!("../data/node_types/c_sharp.txt")),
            "html" => Some(include_str!("../data/node_types/html.txt")),
            "css" => Some(include_str!("../data/node_types/css.txt")),
            "json" => Some(include_str!("../data/node_types/json.txt")),
            "yaml" | "yml" => Some(include_str!("../data/node_types/yaml.txt")),
            "php" => Some(include_str!("../data/node_types/php.txt")),
            "ruby" | "rb" => Some(include_str!("../data/node_types/ruby.txt")),
            "kotlin" | "kt" => Some(include_str!("../data/node_types/kotlin.txt")),
            "scala" => Some(include_str!("../data/node_types/scala.txt")),
            "elixir" | "ex" => Some(include_str!("../data/node_types/elixir.txt")),
            "angular" => Some(include_str!("../data/node_types/angular.txt")),
            _ => None,
        }
    }
}

impl Default for NodeTypesHandler {
    fn default() -> Self {
        Self::new()
    }
}
