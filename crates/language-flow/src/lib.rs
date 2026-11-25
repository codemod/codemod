//! Language-neutral value-flow facts and the queries over them: data flow,
//! mutations and the call graph. A language provider builds [`FileFlow`]s
//! from its own parser and implements [`FlowHost`]; the queries here have no
//! parser in them.

pub mod facts;
pub mod query;

pub use facts::*;
pub use query::{CallerIndex, CallerIndexSlot, FlowHost, QueryKind};
