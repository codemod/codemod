//! Code Knowledge Graph tool

use async_trait::async_trait;
use coro_core::error::Result;
use coro_core::impl_tool_factory;
use coro_core::tools::utils::validate_absolute_path;
use coro_core::tools::{Tool, ToolCall, ToolExample, ToolResult};
use rusqlite::{params, Connection};
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tree_sitter::{Language, Parser, Tree};
use walkdir::WalkDir;

// Language support for tree-sitter
use tree_sitter_c::LANGUAGE as C_LANGUAGE;
use tree_sitter_cpp::LANGUAGE as CPP_LANGUAGE;
use tree_sitter_go::LANGUAGE as GO_LANGUAGE;
use tree_sitter_java::LANGUAGE as JAVA_LANGUAGE;
use tree_sitter_javascript::LANGUAGE as JAVASCRIPT_LANGUAGE;
use tree_sitter_python::LANGUAGE as PYTHON_LANGUAGE;
use tree_sitter_rust::LANGUAGE as RUST_LANGUAGE;
use tree_sitter_typescript::LANGUAGE_TYPESCRIPT as TYPESCRIPT_LANGUAGE;

/// Supported programming languages
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SupportedLanguage {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Java,
    C,
    Cpp,
    Go,
}

impl SupportedLanguage {
    fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "rs" => Some(Self::Rust),
            "py" => Some(Self::Python),
            "js" => Some(Self::JavaScript),
            "ts" => Some(Self::TypeScript),
            "java" => Some(Self::Java),
            "c" => Some(Self::C),
            "cpp" | "cc" | "cxx" => Some(Self::Cpp),
            "go" => Some(Self::Go),
            _ => None,
        }
    }

    fn get_language(&self) -> Language {
        match self {
            Self::Rust => RUST_LANGUAGE.into(),
            Self::Python => PYTHON_LANGUAGE.into(),
            Self::JavaScript => JAVASCRIPT_LANGUAGE.into(),
            Self::TypeScript => TYPESCRIPT_LANGUAGE.into(),
            Self::Java => JAVA_LANGUAGE.into(),
            Self::C => C_LANGUAGE.into(),
            Self::Cpp => CPP_LANGUAGE.into(),
            Self::Go => GO_LANGUAGE.into(),
        }
    }
}

/// Code symbol information
#[derive(Debug, Clone)]
pub struct CodeSymbol {
    pub name: String,
    pub symbol_type: String,
    pub file_path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub start_byte: usize,
    pub end_byte: usize,
    pub parent: Option<String>,
    pub language: String,
}

/// Code Knowledge Graph database
pub struct CkgDatabase {
    connection: Arc<Mutex<Connection>>,
    parsers: HashMap<SupportedLanguage, Parser>,
}

impl CkgDatabase {
    pub fn new(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)?;

        // Create tables
        conn.execute(
            "CREATE TABLE IF NOT EXISTS symbols (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                symbol_type TEXT NOT NULL,
                file_path TEXT NOT NULL,
                start_line INTEGER NOT NULL,
                end_line INTEGER NOT NULL,
                start_byte INTEGER NOT NULL,
                end_byte INTEGER NOT NULL,
                parent TEXT,
                language TEXT NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name)",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_path)",
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_symbols_type ON symbols(symbol_type)",
            [],
        )?;

        let mut parsers = HashMap::new();
        for lang in [
            SupportedLanguage::Rust,
            SupportedLanguage::Python,
            SupportedLanguage::JavaScript,
            SupportedLanguage::TypeScript,
            SupportedLanguage::Java,
            SupportedLanguage::C,
            SupportedLanguage::Cpp,
            SupportedLanguage::Go,
        ] {
            let mut parser = Parser::new();
            parser.set_language(&lang.get_language())?;
            parsers.insert(lang, parser);
        }

        Ok(Self {
            connection: Arc::new(Mutex::new(conn)),
            parsers,
        })
    }

    /// Parse a file and extract symbols
    pub fn parse_file(&mut self, file_path: &Path) -> Result<Vec<CodeSymbol>> {
        let content = std::fs::read_to_string(file_path)?;
        let extension = file_path
            .extension()
            .and_then(|ext| ext.to_str())
            .ok_or("No file extension found")?;

        let language = SupportedLanguage::from_extension(extension)
            .ok_or(format!("Unsupported file extension: {}", extension))?;

        let parser = self
            .parsers
            .get_mut(&language)
            .ok_or("Parser not available for language")?;

        let tree = parser.parse(&content, None).ok_or("Failed to parse file")?;

        self.extract_symbols(&tree, &content, file_path, &language)
    }

    /// Extract symbols from parsed tree
    fn extract_symbols(
        &self,
        tree: &Tree,
        content: &str,
        file_path: &Path,
        language: &SupportedLanguage,
    ) -> Result<Vec<CodeSymbol>> {
        let mut symbols = Vec::new();
        let root_node = tree.root_node();

        // This is a simplified implementation
        // In a full implementation, you would use language-specific queries
        self.traverse_node(root_node, content, file_path, language, &mut symbols, None);

        Ok(symbols)
    }

    /// Recursively traverse AST nodes to extract symbols
    fn traverse_node(
        &self,
        node: tree_sitter::Node,
        content: &str,
        file_path: &Path,
        language: &SupportedLanguage,
        symbols: &mut Vec<CodeSymbol>,
        parent: Option<String>,
    ) {
        let node_type = node.kind();

        // Extract symbol based on node type (simplified)
        if self.is_symbol_node(node_type, language) {
            if let Some(symbol) =
                self.extract_symbol_from_node(node, content, file_path, language, parent.clone())
            {
                symbols.push(symbol);
            }
        }

        // Recursively process children
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.traverse_node(child, content, file_path, language, symbols, parent.clone());
        }
    }

    /// Check if a node type represents a symbol we want to extract
    fn is_symbol_node(&self, node_type: &str, language: &SupportedLanguage) -> bool {
        match language {
            SupportedLanguage::Rust => matches!(
                node_type,
                "function_item"
                    | "struct_item"
                    | "enum_item"
                    | "trait_item"
                    | "impl_item"
                    | "mod_item"
            ),
            SupportedLanguage::Python => {
                matches!(node_type, "function_definition" | "class_definition")
            }
            SupportedLanguage::JavaScript | SupportedLanguage::TypeScript => matches!(
                node_type,
                "function_declaration" | "class_declaration" | "method_definition"
            ),
            SupportedLanguage::Java => matches!(
                node_type,
                "method_declaration" | "class_declaration" | "interface_declaration"
            ),
            SupportedLanguage::C | SupportedLanguage::Cpp => matches!(
                node_type,
                "function_definition" | "struct_specifier" | "class_specifier"
            ),
            SupportedLanguage::Go => matches!(
                node_type,
                "function_declaration" | "type_declaration" | "method_declaration"
            ),
        }
    }

    /// Extract symbol information from a node
    fn extract_symbol_from_node(
        &self,
        node: tree_sitter::Node,
        content: &str,
        file_path: &Path,
        language: &SupportedLanguage,
        parent: Option<String>,
    ) -> Option<CodeSymbol> {
        let name = self.get_symbol_name(node, content)?;
        let symbol_type = node.kind().to_string();

        let start_position = node.start_position();
        let end_position = node.end_position();

        Some(CodeSymbol {
            name,
            symbol_type,
            file_path: file_path.to_string_lossy().to_string(),
            start_line: start_position.row + 1,
            end_line: end_position.row + 1,
            start_byte: node.start_byte(),
            end_byte: node.end_byte(),
            parent,
            language: format!("{:?}", language),
        })
    }

    /// Get symbol name from node
    fn get_symbol_name(&self, node: tree_sitter::Node, content: &str) -> Option<String> {
        // This is a simplified implementation
        // In practice, you'd need language-specific logic to extract names
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "identifier" {
                return Some(content[child.start_byte()..child.end_byte()].to_string());
            }
        }
        None
    }

    /// Store symbols in database
    pub fn store_symbols(&self, symbols: &[CodeSymbol]) -> Result<()> {
        let conn = self
            .connection
            .lock()
            .map_err(|_| "Failed to acquire database lock")?;

        for symbol in symbols {
            conn.execute(
                "INSERT INTO symbols (name, symbol_type, file_path, start_line, end_line, start_byte, end_byte, parent, language)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    symbol.name,
                    symbol.symbol_type,
                    symbol.file_path,
                    symbol.start_line,
                    symbol.end_line,
                    symbol.start_byte,
                    symbol.end_byte,
                    symbol.parent,
                    symbol.language,
                ],
            )?;
        }

        Ok(())
    }

    /// Query symbols from database
    pub fn query_symbols(&self, query: &str) -> Result<Vec<CodeSymbol>> {
        let conn = self
            .connection
            .lock()
            .map_err(|_| "Failed to acquire database lock")?;

        let mut stmt = conn.prepare(
            "SELECT name, symbol_type, file_path, start_line, end_line, start_byte, end_byte, parent, language
             FROM symbols
             WHERE name LIKE ?1 OR symbol_type LIKE ?1 OR file_path LIKE ?1
             ORDER BY name"
        )?;

        let symbol_iter = stmt.query_map([format!("%{}%", query)], |row| {
            Ok(CodeSymbol {
                name: row.get(0)?,
                symbol_type: row.get(1)?,
                file_path: row.get(2)?,
                start_line: row.get(3)?,
                end_line: row.get(4)?,
                start_byte: row.get(5)?,
                end_byte: row.get(6)?,
                parent: row.get(7)?,
                language: row.get(8)?,
            })
        })?;

        let mut symbols = Vec::new();
        for symbol in symbol_iter {
            symbols.push(symbol?);
        }

        Ok(symbols)
    }
}

/// Tool for Code Knowledge Graph operations
pub struct CkgTool {
    database: Arc<Mutex<Option<CkgDatabase>>>,
}

impl CkgTool {
    pub fn new() -> Self {
        Self {
            database: Arc::new(Mutex::new(None)),
        }
    }
}

#[async_trait]
impl Tool for CkgTool {
    fn name(&self) -> &str {
        "ckg_tool"
    }

    fn description(&self) -> &str {
        "Code Knowledge Graph tool for analyzing and querying code structure\n\
         * Parses source code files using tree-sitter to extract symbols\n\
         * Stores code symbols (functions, classes, structs, etc.) in a database\n\
         * Supports multiple programming languages: Rust, Python, JavaScript, TypeScript, Java, C, C++, Go\n\
         * Provides powerful querying capabilities to find symbols by name, type, or file\n\
         * Builds relationships between code elements for better understanding\n\
         \n\
         Operations:\n\
         - `build`: Parse files in a directory and build the knowledge graph\n\
         - `query`: Search for symbols in the knowledge graph\n\
         - `analyze`: Get detailed analysis of a specific file or symbol\n\
         - `stats`: Get statistics about the codebase\n\
         \n\
         Supported file extensions:\n\
         - Rust: .rs\n\
         - Python: .py\n\
         - JavaScript: .js\n\
         - TypeScript: .ts\n\
         - Java: .java\n\
         - C: .c\n\
         - C++: .cpp, .cc, .cxx\n\
         - Go: .go"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["build", "query", "analyze", "stats"],
                    "description": "The operation to perform"
                },
                "path": {
                    "type": "string",
                    "description": "Path to directory (for build) or file (for analyze). Must be absolute path."
                },
                "query": {
                    "type": "string",
                    "description": "Search query for symbols (required for query operation)"
                },
                "db_path": {
                    "type": "string",
                    "description": "Path to the SQLite database file. Defaults to './ckg.db'"
                },
                "recursive": {
                    "type": "boolean",
                    "description": "Whether to recursively process subdirectories (for build operation). Defaults to true."
                },
                "file_extensions": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "File extensions to process (for build operation). If not specified, all supported extensions are used."
                }
            },
            "required": ["operation"]
        })
    }

    async fn execute(&self, call: ToolCall) -> Result<ToolResult> {
        let operation: String = call.get_parameter("operation")?;
        let db_path: String = call.get_parameter_or("db_path", "./ckg.db".to_string());

        // Initialize database if needed
        {
            let mut db_guard = self
                .database
                .lock()
                .map_err(|_| "Failed to acquire database lock")?;
            if db_guard.is_none() {
                *db_guard = Some(CkgDatabase::new(Path::new(&db_path))?);
            }
        }

        match operation.as_str() {
            "build" => {
                let path: String = call.get_parameter("path")?;
                let recursive: bool = call.get_parameter_or("recursive", true);
                let file_extensions: Option<Vec<String>> =
                    call.get_parameter("file_extensions").ok();

                self.build_knowledge_graph(&call.id, &path, recursive, file_extensions)
                    .await
            }
            "query" => {
                let query: String = call.get_parameter("query")?;
                self.query_symbols(&call.id, &query).await
            }
            "analyze" => {
                let path: String = call.get_parameter("path")?;
                self.analyze_file(&call.id, &path).await
            }
            "stats" => self.get_statistics(&call.id).await,
            _ => Ok(ToolResult::error(
                &call.id,
                &format!(
                    "Unknown operation: {}. Supported operations: build, query, analyze, stats",
                    operation
                ),
            )),
        }
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![
            ToolExample {
                description: "Build knowledge graph from a directory".to_string(),
                parameters: json!({
                    "operation": "build",
                    "path": "/project/src",
                    "recursive": true
                }),
                expected_result: "Knowledge graph built successfully".to_string(),
            },
            ToolExample {
                description: "Query symbols by name".to_string(),
                parameters: json!({
                    "operation": "query",
                    "query": "main"
                }),
                expected_result: "List of symbols matching 'main'".to_string(),
            },
            ToolExample {
                description: "Analyze a specific file".to_string(),
                parameters: json!({
                    "operation": "analyze",
                    "path": "/project/src/main.rs"
                }),
                expected_result: "Detailed analysis of the file".to_string(),
            },
            ToolExample {
                description: "Get codebase statistics".to_string(),
                parameters: json!({
                    "operation": "stats"
                }),
                expected_result: "Statistics about the codebase".to_string(),
            },
        ]
    }
}

impl CkgTool {
    /// Build knowledge graph from directory
    async fn build_knowledge_graph(
        &self,
        call_id: &str,
        path: &str,
        recursive: bool,
        file_extensions: Option<Vec<String>>,
    ) -> Result<ToolResult> {
        let path = Path::new(path);
        validate_absolute_path(path)?;

        if !path.exists() {
            return Ok(ToolResult::error(
                call_id,
                &format!("Path does not exist: {}", path.display()),
            ));
        }

        if !path.is_dir() {
            return Ok(ToolResult::error(
                call_id,
                &format!("Path is not a directory: {}", path.display()),
            ));
        }

        let extensions = file_extensions.unwrap_or_else(|| {
            vec![
                "rs", "py", "js", "ts", "java", "c", "cpp", "cc", "cxx", "go",
            ]
            .into_iter()
            .map(|s| s.to_string())
            .collect()
        });

        let mut total_files = 0;
        let mut processed_files = 0;
        let mut total_symbols = 0;
        let mut errors = Vec::new();

        let walker = if recursive {
            WalkDir::new(path)
        } else {
            WalkDir::new(path).max_depth(1)
        };

        for entry in walker {
            match entry {
                Ok(entry) => {
                    if entry.file_type().is_file() {
                        if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
                            if extensions.contains(&ext.to_string()) {
                                total_files += 1;

                                match self.process_file(entry.path()).await {
                                    Ok(symbol_count) => {
                                        processed_files += 1;
                                        total_symbols += symbol_count;
                                    }
                                    Err(e) => {
                                        errors.push(format!(
                                            "Error processing {}: {}",
                                            entry.path().display(),
                                            e
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    errors.push(format!("Error walking directory: {}", e));
                }
            }
        }

        let mut result = format!(
            "Knowledge graph build completed!\n\
             Total files found: {}\n\
             Files processed: {}\n\
             Total symbols extracted: {}",
            total_files, processed_files, total_symbols
        );

        if !errors.is_empty() {
            result.push_str(&format!("\n\nErrors encountered ({}):\n", errors.len()));
            for (i, error) in errors.iter().take(10).enumerate() {
                result.push_str(&format!("{}. {}\n", i + 1, error));
            }
            if errors.len() > 10 {
                result.push_str(&format!("... and {} more errors\n", errors.len() - 10));
            }
        }

        Ok(ToolResult::success(call_id, &result))
    }

    /// Process a single file
    async fn process_file(&self, file_path: &Path) -> Result<usize> {
        let mut db_guard = self
            .database
            .lock()
            .map_err(|_| "Failed to acquire database lock")?;
        let database = db_guard.as_mut().ok_or("Database not initialized")?;

        let symbols = database.parse_file(file_path)?;
        let symbol_count = symbols.len();
        database.store_symbols(&symbols)?;

        Ok(symbol_count)
    }

    /// Query symbols from the knowledge graph
    async fn query_symbols(&self, call_id: &str, query: &str) -> Result<ToolResult> {
        let db_guard = self
            .database
            .lock()
            .map_err(|_| "Failed to acquire database lock")?;
        let database = db_guard.as_ref().ok_or("Database not initialized")?;

        let symbols = database.query_symbols(query)?;

        if symbols.is_empty() {
            return Ok(ToolResult::success(
                call_id,
                &format!("No symbols found matching query: '{}'", query),
            ));
        }

        let mut result = format!(
            "Found {} symbols matching query '{}':\n\n",
            symbols.len(),
            query
        );

        for (i, symbol) in symbols.iter().take(50).enumerate() {
            result.push_str(&format!(
                "{}. {} ({})\n   File: {}:{}:{}\n   Type: {}\n",
                i + 1,
                symbol.name,
                symbol.language,
                symbol.file_path,
                symbol.start_line,
                symbol.end_line,
                symbol.symbol_type
            ));

            if let Some(parent) = &symbol.parent {
                result.push_str(&format!("   Parent: {}\n", parent));
            }
            result.push('\n');
        }

        if symbols.len() > 50 {
            result.push_str(&format!("... and {} more symbols\n", symbols.len() - 50));
        }

        Ok(ToolResult::success(call_id, &result))
    }

    /// Analyze a specific file
    async fn analyze_file(&self, call_id: &str, path: &str) -> Result<ToolResult> {
        let path = Path::new(path);
        validate_absolute_path(path)?;

        if !path.exists() {
            return Ok(ToolResult::error(
                call_id,
                &format!("File does not exist: {}", path.display()),
            ));
        }

        if !path.is_file() {
            return Ok(ToolResult::error(
                call_id,
                &format!("Path is not a file: {}", path.display()),
            ));
        }

        let mut db_guard = self
            .database
            .lock()
            .map_err(|_| "Failed to acquire database lock")?;
        let database = db_guard.as_mut().ok_or("Database not initialized")?;

        let symbols = database.parse_file(path)?;

        let mut result = format!("Analysis of {}:\n\n", path.display());
        result.push_str(&format!("Total symbols found: {}\n\n", symbols.len()));

        // Group symbols by type
        let mut symbol_types: HashMap<String, Vec<&CodeSymbol>> = HashMap::new();
        for symbol in &symbols {
            symbol_types
                .entry(symbol.symbol_type.clone())
                .or_default()
                .push(symbol);
        }

        for (symbol_type, symbols_of_type) in symbol_types {
            result.push_str(&format!("{}s ({}):\n", symbol_type, symbols_of_type.len()));
            for symbol in symbols_of_type.iter().take(20) {
                result.push_str(&format!(
                    "  - {} (lines {}-{})\n",
                    symbol.name, symbol.start_line, symbol.end_line
                ));
            }
            if symbols_of_type.len() > 20 {
                result.push_str(&format!("  ... and {} more\n", symbols_of_type.len() - 20));
            }
            result.push('\n');
        }

        Ok(ToolResult::success(call_id, &result))
    }

    /// Get statistics about the codebase
    async fn get_statistics(&self, call_id: &str) -> Result<ToolResult> {
        let db_guard = self
            .database
            .lock()
            .map_err(|_| "Failed to acquire database lock")?;
        let database = db_guard.as_ref().ok_or("Database not initialized")?;

        let conn = database
            .connection
            .lock()
            .map_err(|_| "Failed to acquire database connection")?;

        // Get total symbol count
        let total_symbols: i64 =
            conn.query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get(0))?;

        // Get symbols by type
        let mut stmt = conn.prepare(
            "SELECT symbol_type, COUNT(*) FROM symbols GROUP BY symbol_type ORDER BY COUNT(*) DESC",
        )?;
        let type_counts: std::result::Result<Vec<(String, i64)>, rusqlite::Error> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect();
        let type_counts = type_counts?;

        // Get symbols by language
        let mut stmt = conn.prepare(
            "SELECT language, COUNT(*) FROM symbols GROUP BY language ORDER BY COUNT(*) DESC",
        )?;
        let lang_counts: std::result::Result<Vec<(String, i64)>, rusqlite::Error> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect();
        let lang_counts = lang_counts?;

        // Get file count
        let file_count: i64 =
            conn.query_row("SELECT COUNT(DISTINCT file_path) FROM symbols", [], |row| {
                row.get(0)
            })?;

        let mut result = "Code Knowledge Graph Statistics:\n\n".to_string();
        result.push_str(&format!("Total symbols: {}\n", total_symbols));
        result.push_str(&format!("Total files: {}\n\n", file_count));

        result.push_str("Symbols by type:\n");
        for (symbol_type, count) in type_counts {
            result.push_str(&format!("  {}: {}\n", symbol_type, count));
        }

        result.push_str("\nSymbols by language:\n");
        for (language, count) in lang_counts {
            result.push_str(&format!("  {}: {}\n", language, count));
        }

        Ok(ToolResult::success(call_id, &result))
    }
}

impl_tool_factory!(
    CkgToolFactory,
    CkgTool,
    "ckg_tool",
    "Code Knowledge Graph tool for analyzing and querying code structure"
);
