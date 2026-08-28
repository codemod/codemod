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

    pub fn get_node_types_text(&self, language: &str) -> Result<String, String> {
        let node_types = self
            .get_node_types_for_language(language)
            .ok_or_else(|| format!("Unsupported language '{language}'"))?;

        Ok(format_node_types_response(node_types))
    }

    #[tool(
        description = "Get tree-sitter node types for a specific programming language in AI-friendly format. You should use this tool to get the node types for the language you are working in. You do not know the node types for the language you are working in, so you should use this tool to get them."
    )]
    pub async fn get_node_types(
        &self,
        Parameters(request): Parameters<GetNodeTypesRequest>,
    ) -> Result<CallToolResult, McpError> {
        let result = self
            .get_node_types_text(&request.language)
            .map_err(|error| McpError::invalid_params(error, None))?;

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
            "xml" => Some(include_str!("../data/node_types/xml.txt")),
            "css" => Some(include_str!("../data/node_types/css.txt")),
            "json" => Some(include_str!("../data/node_types/json.txt")),
            "yaml" | "yml" => Some(include_str!("../data/node_types/yaml.txt")),
            "toml" => Some(include_str!("../data/node_types/toml.txt")),
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

fn format_node_types_response(node_types: &str) -> String {
    format!(
        "<TREE_SITTER_NODE_TYPES>\n\
{node_types}\n\
</TREE_SITTER_NODE_TYPES>\n\
\n\
<LEGEND>\n\
Legends for field notation:\n\
- `?` - optional field (may not be present in all instances)\n\
- `*` - multiple values allowed (array/list of values)\n\
\n\
In tree-sitter grammar:\n\
- Fields are named children with specific roles in the syntax tree\n\
- Format: `fieldName=nodeType` (e.g., \"body=block\")\n\
- When a field is not named, it's represented as `children=nodeType`\n\
- Multiple possible types are comma-separated (e.g., \"value=string,number\")\n\
</LEGEND>\n"
    )
}

impl Default for NodeTypesHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{format_node_types_response, NodeTypesHandler};

    #[test]
    fn xml_language_dispatch_returns_node_types() {
        let handler = NodeTypesHandler::new();
        let xml = handler
            .get_node_types_for_language("xml")
            .expect("xml node types should exist");
        assert!(xml.contains("document:"));
    }

    #[test]
    fn unsupported_language_returns_none() {
        let handler = NodeTypesHandler::new();
        assert!(handler
            .get_node_types_for_language("not-a-language")
            .is_none());
    }

    #[test]
    fn formatted_node_types_tags_are_flush_left() {
        let response = format_node_types_response("program:");
        assert!(response.starts_with("<TREE_SITTER_NODE_TYPES>\nprogram:"));
        assert!(response.contains("\n</TREE_SITTER_NODE_TYPES>\n\n<LEGEND>"));
    }
}
