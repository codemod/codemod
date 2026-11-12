//! workflow.yaml compatibility: translates a YAML workflow into the
//! equivalent TypeScript workflow.

/// A translated workflow.
#[derive(Debug, Clone)]
pub struct Translated {
    pub source: String,
    pub warnings: Vec<String>,
}

/// Translate workflow YAML (or JSON) source.
pub fn translate_source(_source: &str, path: &std::path::Path) -> Result<Translated, String> {
    Err(format!(
        "{}: running workflow.yaml is not supported yet",
        path.display()
    ))
}
