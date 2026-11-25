//! How the JavaScript analyzer reads model facts.
//!
//! The facts and the graph are language-neutral (`language_core::ModelStore`,
//! `ModelGraph`). This is the part that knows JavaScript: which function a
//! fact's node stands for, which function a call site is in, and which
//! value-flow node a range is. Nodes in files this analyzer cannot parse
//! (another language) are left to the graph, which takes what the fact says.

use crate::flow_query::{analyzable, functions_at, FlowHost};
use language_core::{ByteRange, ModelFunction, ModelNode, ModelResolver};
use std::path::{Path, PathBuf};

pub(crate) struct JsResolver<'a>(pub &'a dyn FlowHost);

impl ModelResolver for JsResolver<'_> {
    fn canonical(&self, path: &Path) -> PathBuf {
        self.0.canonical(path)
    }

    /// The function, its name, its export statement, or a reference to it.
    fn functions(&self, node: &ModelNode) -> Option<Vec<ModelFunction>> {
        let path = self.0.canonical(&node.file);
        analyzable(&path).then(|| self.0.flow(&path))??;
        let found = functions_at(self.0, &path, node.range(), true)
            .into_iter()
            .filter_map(|(file, function)| {
                let flow = self.0.flow(&file)?;
                let fact = flow.functions.get(function? as usize)?;
                Some(ModelFunction {
                    range: fact.range,
                    name: fact.name.as_deref().map(str::to_string),
                    file,
                })
            })
            .collect();
        Some(found)
    }

    /// The function a call a model added is made from: the function itself
    /// when the site is one, else the function around it.
    fn enclosing(&self, node: &ModelNode) -> Option<Option<ModelFunction>> {
        let path = self.0.canonical(&node.file);
        let flow = analyzable(&path).then(|| self.0.flow(&path))??;
        let range = node.range();
        let index = flow
            .functions
            .iter()
            .position(|f| f.range == range || f.name_range == Some(range))
            .map(|index| index as u32)
            .or_else(|| flow.function_at(range));
        Some(index.map(|index| {
            let fact = &flow.functions[index as usize];
            ModelFunction {
                file: path,
                range: fact.range,
                name: fact.name.as_deref().map(str::to_string),
            }
        }))
    }

    /// The range of the value-flow node (a fact's range comes from another
    /// parser and may sit a token wider).
    fn value(&self, node: &ModelNode) -> Option<ByteRange> {
        let path = self.0.canonical(&node.file);
        let flow = analyzable(&path).then(|| self.0.flow(&path))??;
        let found = flow.locate(node.range())?;
        Some(flow.nodes[found as usize].range)
    }
}
