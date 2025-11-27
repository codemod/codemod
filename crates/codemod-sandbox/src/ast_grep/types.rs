use rquickjs::{Ctx, Error as QError, FromJs, IntoJs, Object, Result, Value};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct JsPosition {
    pub line: usize,
    pub column: usize,
    pub index: usize,
}

impl<'js> FromJs<'js> for JsPosition {
    fn from_js(_ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        let ty_name = value.type_name();
        let obj = value
            .as_object()
            .ok_or(QError::new_from_js(ty_name, "Object"))?;
        let line = obj.get("line")?;
        let column = obj.get("column")?;
        let index = obj.get("index")?;
        Ok(Self {
            line,
            column,
            index,
        })
    }
}

impl<'js> IntoJs<'js> for JsPosition {
    fn into_js(self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let obj = Object::new(ctx.clone())?;
        obj.set("line", self.line)?;
        obj.set("column", self.column)?;
        obj.set("index", self.index)?;
        obj.into_js(ctx)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct JsNodeRange {
    pub start: JsPosition,
    pub end: JsPosition,
}

impl<'js> FromJs<'js> for JsNodeRange {
    fn from_js(_ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        let obj = value
            .as_object()
            .ok_or(QError::new_from_js(value.type_name(), "Object"))?;
        let start = obj.get("start")?;
        let end = obj.get("end")?;
        Ok(Self { start, end })
    }
}

impl<'js> IntoJs<'js> for JsNodeRange {
    fn into_js(self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let obj = Object::new(ctx.clone())?;
        obj.set("start", self.start)?;
        obj.set("end", self.end)?;
        obj.into_js(ctx)
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct JsEdit {
    #[serde(rename = "startPos")]
    pub start_pos: u32,
    #[serde(rename = "endPos")]
    pub end_pos: u32,
    #[serde(rename = "insertedText")]
    pub inserted_text: String,
}
impl<'js> FromJs<'js> for JsEdit {
    fn from_js(_ctx: &Ctx<'js>, value: Value<'js>) -> Result<Self> {
        let obj = value
            .as_object()
            .ok_or(QError::new_from_js(value.type_name(), "Object"))?;
        let start_pos = obj.get("startPos")?;
        let end_pos = obj.get("endPos")?;
        let inserted_text = obj.get("insertedText")?;
        Ok(Self {
            start_pos,
            end_pos,
            inserted_text,
        })
    }
}

impl<'js> IntoJs<'js> for JsEdit {
    fn into_js(self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let obj = Object::new(ctx.clone())?;
        obj.set("startPos", self.start_pos)?;
        obj.set("endPos", self.end_pos)?;
        obj.set("insertedText", self.inserted_text)?;
        obj.into_js(ctx)
    }
}

#[derive(Error, Debug)]
pub enum AstGrepError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[cfg(feature = "native")]
    #[error("YAML error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("Language error: {0}")]
    Language(String),
    #[error("Config error: {0}")]
    Config(String),
    #[error("Path error: {0}")]
    Path(String),
    #[error("Glob error: {0}")]
    Glob(String),
}

#[derive(Debug, Clone)]
pub struct AstGrepMatch {
    pub file_path: String,
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
    pub match_text: String,
    pub rule_id: String,
}

/// JavaScript-exposed symbol location for semantic analysis results.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct JsSymbolLocation {
    pub file_path: String,
    pub start: u32,
    pub end: u32,
    pub kind: String,
    pub name: String,
}

impl<'js> IntoJs<'js> for JsSymbolLocation {
    fn into_js(self, ctx: &Ctx<'js>) -> Result<Value<'js>> {
        let obj = Object::new(ctx.clone())?;
        obj.set("filePath", self.file_path)?;
        obj.set("start", self.start)?;
        obj.set("end", self.end)?;
        obj.set("kind", self.kind)?;
        obj.set("name", self.name)?;
        obj.into_js(ctx)
    }
}

#[cfg(feature = "native")]
impl From<language_core::SymbolLocation> for JsSymbolLocation {
    fn from(loc: language_core::SymbolLocation) -> Self {
        Self {
            file_path: loc.file_path.to_string_lossy().to_string(),
            start: loc.range.start,
            end: loc.range.end,
            kind: loc.kind.as_str().to_string(),
            name: loc.name,
        }
    }
}
