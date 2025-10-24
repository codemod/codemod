use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use std::collections::HashMap;
use ts_rs::TS;

/// Simple schema system for workflow state and params validation
/// Root is always an object with properties
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema, TS)]
pub struct SimpleSchema {
    /// Properties of the root object
    #[serde(flatten)]
    pub properties: HashMap<String, SimpleSchemaProperty>,
}

/// Represents a property in the schema with common metadata and type-specific fields
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
pub struct SimpleSchemaProperty {
    /// Human-readable name for this property
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// Description of what this property represents
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// The actual schema definition
    #[serde(flatten)]
    pub schema: SimpleSchemaType,
}

/// Represents the type-specific schema definition
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SimpleSchemaType {
    /// String type with optional oneOf and default
    String {
        /// Allows multiple schema alternatives for strings
        #[serde(rename = "oneOf")]
        one_of: Option<Vec<SimpleSchemaVariant>>,

        /// Default value for the property
        default: Option<String>,

        /// Whether the string is multi-line
        multi_line: Option<bool>,

        /// Whether the string is a secret
        secret: Option<bool>,
    },

    Number {
        /// Default value for the property
        default: Option<Number>,
    },

    /// Array type with required items schema
    Array {
        /// Defines the schema of array items
        items: Box<SimpleSchemaProperty>,

        /// Default value for the property
        default: Option<String>,
    },

    /// Object type with properties
    Object {
        /// Properties of the object
        properties: Option<HashMap<String, SimpleSchemaProperty>>,

        /// Default value for the property
        default: Option<String>,
    },

    /// Boolean type with optional default
    Boolean {
        /// Default value for the property
        default: Option<bool>,
    },
}

/// Represents a variant in a oneOf schema for strings
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, TS)]
pub struct SimpleSchemaVariant {
    /// Type of this variant (always "string" for oneOf variants)
    #[serde(rename = "type")]
    pub schema_type: String,

    /// For string types with enumeration, the allowed values
    #[serde(rename = "enum")]
    pub enum_values: Option<Vec<String>>,
}

pub fn resolve_values_with_default(
    schema: &SimpleSchema,
    values: &HashMap<String, Value>,
) -> HashMap<String, Value> {
    let mut resolved_values = values.clone();
    for (key, property) in schema.properties.iter() {
        if resolved_values.contains_key(key) {
            continue;
        }
        let default = match &property.schema {
            SimpleSchemaType::String {
                default: Some(default),
                ..
            } => Value::String(default.clone()),
            SimpleSchemaType::Number {
                default: Some(default),
                ..
            } => Value::Number(default.clone()),
            SimpleSchemaType::Boolean {
                default: Some(default),
                ..
            } => Value::Bool(*default),
            _ => continue,
        };
        resolved_values.insert(key.clone(), default);
    }
    resolved_values
}
