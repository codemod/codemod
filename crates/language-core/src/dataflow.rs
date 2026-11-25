//! Data-flow and call-graph types shared by semantic providers.
//!
//! These are language-neutral: a provider answers with byte ranges grouped
//! by file, and the sandbox layer turns them into `SgNode`s. Everything here
//! is a "may" answer: a provider that cannot follow a value any further says
//! so (`FlowStep::boundary`, `CallGraphEntry::resolved == false`) instead of
//! guessing or silently dropping the edge.

use crate::ByteRange;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

macro_rules! str_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name {
            $($(#[$vmeta])* #[serde(rename = $text)] $variant,)+
        }

        impl $name {
            /// The name JavaScript callers see.
            pub fn as_str(&self) -> &'static str {
                match self {
                    $($name::$variant => $text,)+
                }
            }
        }
    };
}

str_enum! {
    /// What a node of the value-flow graph is.
    FlowNodeKind {
        /// A local binding (`const`, `let`, `var`, function or class name, catch parameter).
        Variable => "variable",
        /// A function parameter. The value comes from the callers.
        Parameter => "parameter",
        /// An imported binding. The value comes from another module.
        Import => "import",
        /// A read of a binding.
        Reference => "reference",
        /// A function or arrow function value.
        Function => "function",
        /// A class value.
        Class => "class",
        /// The result of a call or `new`.
        Call => "call",
        /// A property read (`a.b`, `a[b]`), or the target of a property write.
        Member => "member",
        /// A literal.
        Literal => "literal",
        /// A name with no binding in the module (a global).
        Global => "global",
        /// `this`.
        This => "this",
        /// An object literal.
        Object => "object",
        /// An array literal.
        Array => "array",
        /// A JSX element or fragment.
        Jsx => "jsx",
        /// A JSX attribute.
        JsxAttribute => "jsx-attribute",
        /// Any other expression.
        Expression => "expression",
    }
}

str_enum! {
    /// How a value gets from one node of the value-flow graph to the next.
    FlowEdgeKind {
        /// Initializer or assignment into a binding.
        Assign => "assign",
        /// A binding read by a reference.
        Read => "read",
        /// The same value under another expression (`(x)`, `x as T`, `a ?? b`, `c ? a : b`).
        Alias => "alias",
        /// A property of the value (`x.p`, `const { p } = x`).
        Property => "property",
        /// An element of the value (`x[i]`, `for (const e of x)`, `const [e] = x`).
        Element => "element",
        /// The value is an argument of the call.
        Argument => "argument",
        /// The value is the receiver of the method call.
        Receiver => "receiver",
        /// Computed from the value (`x + 1`, `\`${x}\``, `!x`, `await x`).
        Derive => "derive",
        /// Stored inside a new object or array (`{ p: x }`, `[x]`, `...x`).
        Capture => "capture",
        /// Written to a property (`o.p = x`).
        Store => "store",
        /// The value is a JSX attribute's value.
        JsxAttribute => "jsx-attribute",
        /// The value is a JSX child.
        JsxChild => "jsx-child",
        /// The value is returned from the function.
        Return => "return",
    }
}

/// Which way to follow values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlowDirection {
    /// What the value depends on.
    #[default]
    Backward,
    /// Where the value goes.
    Forward,
}

/// Options for a data-flow query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlowOptions {
    pub direction: FlowDirection,
    /// Treat the result of a call as derived from its arguments and receiver.
    /// This is the conservative answer for calls with no summary. Default: true.
    pub through_calls: bool,
    /// Stop after this many edges.
    pub max_depth: Option<u32>,
}

impl Default for FlowOptions {
    fn default() -> Self {
        Self {
            direction: FlowDirection::Backward,
            through_calls: true,
            max_depth: None,
        }
    }
}

/// One node reached by a data-flow query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowStep {
    pub range: ByteRange,
    pub kind: FlowNodeKind,
    /// The edge that reached this node.
    pub via: FlowEdgeKind,
    /// Binding, property, attribute or callee name, when the node has one.
    pub name: Option<String>,
    /// Parameter position (for parameters) or argument position (for `via: argument`).
    pub index: Option<u32>,
    /// Number of edges from the queried node.
    pub depth: u32,
    /// The analysis stops here because the value comes from outside what was
    /// analyzed: a parameter, an import, a global, `this`, or a call result.
    pub boundary: bool,
}

/// Steps in one file, with its content so callers can build a syntax tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFlowSteps {
    pub file_path: PathBuf,
    pub content: String,
    pub steps: Vec<FlowStep>,
}

/// Result of a data-flow query.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowResult {
    pub files: Vec<FileFlowSteps>,
    /// A limit was hit and steps are missing.
    pub truncated: bool,
}

/// Which way to walk the call graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CallGraphDirection {
    /// Functions called from the queried function.
    #[default]
    Callees,
    /// Functions that call the queried function.
    Callers,
}

/// Options for a call-graph query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CallGraphOptions {
    pub direction: CallGraphDirection,
    /// Follow calls of calls.
    pub transitive: bool,
    pub max_depth: Option<u32>,
}

str_enum! {
    /// Why the call graph has an edge.
    CallEdgeKind {
        /// `f()`.
        Direct => "direct",
        /// `o.m()`, `this.m()`, `ns.f()`.
        Method => "method",
        /// `new C()`.
        Constructor => "constructor",
        /// The function is passed to a call the analysis cannot see into
        /// (`items.map(f)`, `setTimeout(f)`) and is assumed to be called.
        Callback => "callback",
        /// `<Component />`.
        Jsx => "jsx",
    }
}

/// One function reached by a call-graph query, or one call that could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallGraphEntry {
    /// The function's range. For an unresolved call, or a call made from
    /// module top-level code, the call's range.
    pub range: ByteRange,
    /// Function name, or the callee's text for an unresolved call.
    pub name: Option<String>,
    /// False when the callee is not a function in the workspace (a library,
    /// a builtin, or a value the analysis lost track of).
    pub resolved: bool,
    pub via: CallEdgeKind,
    /// Number of calls from the queried function.
    pub depth: u32,
    /// The call that made this edge.
    pub call_site: Option<(PathBuf, ByteRange)>,
}

/// Call-graph entries in one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallGraphFile {
    pub file_path: PathBuf,
    pub content: String,
    pub entries: Vec<CallGraphEntry>,
}

/// A file the analysis could not parse. Calls into it or from it are missing
/// from the answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnparsedFile {
    pub file_path: PathBuf,
    pub message: String,
}

/// Result of a call-graph query.
///
/// Callers come from a table of every call in the workspace, built on the
/// first callers query and kept for the provider's lifetime: a snapshot of
/// the workspace at that moment. Files edited later keep their old calls in
/// it until the provider's cache is cleared.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallGraphResult {
    pub files: Vec<CallGraphFile>,
    pub truncated: bool,
    /// Files the walk needed but could not parse: imports of the functions
    /// it visited (callees), or workspace files (callers).
    pub unparsed: Vec<UnparsedFile>,
}

str_enum! {
    /// How a binding, or the value it holds, is changed.
    MutationKind {
        /// `x = …`.
        Reassign => "reassign",
        /// `x++`, `x.p--`.
        Update => "update",
        /// `x.p = …`.
        PropertyWrite => "property-write",
        /// `delete x.p`.
        Delete => "delete",
        /// `x.m(…)`.
        MethodCall => "method-call",
        /// `f(x)`: the callee may change it.
        Escape => "escape",
    }
}

str_enum! {
    /// How sure the analysis is that a mutation happens.
    MutationConfidence {
        /// A write in the syntax.
        Definite => "definite",
        /// A call of a method known to mutate its receiver (`push`, `set`, …).
        Likely => "likely",
        /// A call the analysis has no summary for.
        Maybe => "maybe",
    }
}

/// One place that changes a binding or its value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mutation {
    pub range: ByteRange,
    pub kind: MutationKind,
    pub confidence: MutationConfidence,
    /// Property or method name.
    pub name: Option<String>,
    /// The mutation sits in a nested function, so it runs whenever that
    /// function is called rather than where it is written.
    pub deferred: bool,
    /// The mutation goes through another binding that holds the same value.
    pub via_alias: bool,
}

/// Options for a mutation query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MutationOptions {
    /// Only mutations that can run after this range.
    pub after: Option<ByteRange>,
}

/// Result of a mutation query. Mutations are found within the queried file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MutationsResult {
    pub mutations: Vec<Mutation>,
}
