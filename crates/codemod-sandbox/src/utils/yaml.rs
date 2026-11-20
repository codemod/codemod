//! YAML files imported as modules: `import rules from "./rules.yaml"` gives
//! the file's content as JSON, an array when it has several documents.

use serde::Deserialize;
use serde_json::Value;

/// Whether a module path names a YAML file.
pub fn is_yaml(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".yaml") || lower.ends_with(".yml")
}

/// A YAML file's content as JSON: its document, or an array of its
/// documents when it has more than one (as in a multi-rule ast-grep file).
/// Empty documents are left out.
pub fn yaml_to_json(text: &str) -> Result<Value, String> {
    let mut documents = Vec::new();
    for document in serde_yaml::Deserializer::from_str(text) {
        let value = Value::deserialize(document).map_err(|e| e.to_string())?;
        if !value.is_null() {
            documents.push(value);
        }
    }
    Ok(match documents.len() {
        0 => Value::Null,
        1 => documents.pop().unwrap_or(Value::Null),
        _ => Value::Array(documents),
    })
}

/// The JavaScript module a YAML file is loaded as.
pub fn yaml_module(text: &str) -> Result<String, String> {
    Ok(format!("export default {};\n", yaml_to_json(text)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn one_document_is_its_value_and_several_an_array() {
        assert_eq!(
            yaml_to_json("id: a\nrule:\n  pattern: x\n").unwrap(),
            json!({ "id": "a", "rule": { "pattern": "x" } })
        );
        assert_eq!(
            yaml_to_json("id: a\n---\nid: b\n---\n").unwrap(),
            json!([{ "id": "a" }, { "id": "b" }])
        );
        assert!(yaml_to_json("id: [").is_err());
        assert_eq!(
            yaml_module("id: a").unwrap(),
            "export default {\"id\":\"a\"};\n"
        );
    }
}
