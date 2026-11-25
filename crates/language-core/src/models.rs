//! Model facts: what a framework model adds to the analysis.
//!
//! A model is code outside the engine (a JSSG module) that looks at a file
//! and states facts the analysis cannot see in the syntax: "the framework
//! calls this function", "this call sends an HTTP request to that route".
//! Providers merge the facts into their ordinary answers (`callers()`,
//! `callees()`, `flowsTo()`, `flowsFrom()`), and every merged edge names the
//! model it came from.
//!
//! Everything here is language-neutral: a node is a file and a byte range.

use crate::ByteRange;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A syntax node a fact is about.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelNode {
    pub file: PathBuf,
    pub start: u32,
    pub end: u32,
}

impl ModelNode {
    pub fn range(&self) -> ByteRange {
        ByteRange::new(self.start, self.end)
    }
}

/// Which end of a channel a node is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelSide {
    /// Sends: an HTTP request, a published message, a written column.
    Producer,
    /// Receives: a route handler, a subscriber, a read of the column.
    Consumer,
}

/// One part of a channel key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum KeyPart {
    /// Fixed text. Compared exactly; models normalize case and separators.
    Literal(String),
    /// One part whose value varies: a route parameter (`[id]`, `:id`) on the
    /// consumer, an interpolated expression on the producer. Names the port
    /// that carries the value (empty: none).
    Hole { hole: String },
    /// One or more parts to the end of the key (`[...slug]`, `*`).
    Rest { rest: String },
    /// The model could not work out this part or anything after it
    /// (a URL built by a function it cannot see into).
    Unknown { unknown: bool },
}

/// What producers and consumers are joined on.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChannelKey {
    /// `http`, `kafka`, `graphql`, `sql`, `env`, … Only keys of the same
    /// protocol are compared.
    pub protocol: String,
    pub parts: Vec<KeyPart>,
    /// The key as its protocol writes it (`GET /api/users/[id]`), for
    /// people. Not compared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl ChannelKey {
    /// For people: `http GET /api/users/[id]`.
    pub fn display(&self) -> String {
        if let Some(text) = &self.text {
            return format!("{} {text}", self.protocol);
        }
        let parts: Vec<String> = self
            .parts
            .iter()
            .map(|part| match part {
                KeyPart::Literal(literal) => literal.clone(),
                KeyPart::Hole { hole } => format!("{{{hole}}}"),
                KeyPart::Rest { rest } => format!("{{...{rest}}}"),
                KeyPart::Unknown { .. } => "?".to_string(),
            })
            .collect();
        format!("{} {}", self.protocol, parts.join("/"))
    }

    /// True when a part is unknown: the key may stand for many keys.
    pub fn is_open(&self) -> bool {
        self.parts
            .iter()
            .any(|part| matches!(part, KeyPart::Unknown { .. }))
    }
}

/// How well two keys match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyMatch {
    /// Every part is a literal equal to a literal, or a literal or hole
    /// filling a consumer hole. False when the producer has a varying part
    /// where the consumer has fixed text (it may or may not be that text).
    pub exact: bool,
    /// A part was unknown: the keys are compatible, nothing more.
    pub open: bool,
    /// Ports joined by position: (producer port, consumer port).
    pub bindings: Vec<(String, String)>,
}

/// Whether a producer's key can reach a consumer's key.
pub fn match_keys(producer: &ChannelKey, consumer: &ChannelKey) -> Option<KeyMatch> {
    if producer.protocol != consumer.protocol {
        return None;
    }
    let mut result = KeyMatch {
        exact: true,
        open: false,
        bindings: Vec::new(),
    };
    let mut index = 0;
    loop {
        match (producer.parts.get(index), consumer.parts.get(index)) {
            (None, None) => return Some(result),
            (Some(KeyPart::Unknown { .. }), _) | (_, Some(KeyPart::Unknown { .. })) => {
                result.open = true;
                result.exact = false;
                return Some(result);
            }
            // A rest part takes what is left, and needs at least one part.
            (Some(KeyPart::Rest { .. }), Some(_)) => {
                result.exact = false;
                return Some(result);
            }
            (Some(_), Some(KeyPart::Rest { .. })) => return Some(result),
            (None, _) | (_, None) => return None,
            (Some(KeyPart::Literal(a)), Some(KeyPart::Literal(b))) => {
                if a != b {
                    return None;
                }
            }
            (Some(KeyPart::Hole { hole: sent }), Some(KeyPart::Hole { hole: received })) => {
                if !sent.is_empty() && !received.is_empty() {
                    result.bindings.push((sent.clone(), received.clone()));
                }
            }
            (Some(KeyPart::Literal(_)), Some(KeyPart::Hole { .. })) => {}
            (Some(KeyPart::Hole { .. }), Some(KeyPart::Literal(_))) => result.exact = false,
        }
        index += 1;
    }
}

/// A named value that crosses a channel.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelPort {
    pub name: String,
    pub node: ModelNode,
}

/// One end of a channel.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChannelFact {
    pub side: ChannelSide,
    pub key: ChannelKey,
    /// Producer: the call that sends. Consumer: the function that handles.
    pub node: ModelNode,
    /// Values this end sends to the other (producer: path parameters, body;
    /// consumer: the response).
    #[serde(default)]
    pub sends: Vec<ModelPort>,
    /// Where this end receives the other's values.
    #[serde(default)]
    pub receives: Vec<ModelPort>,
}

/// One fact.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ModelFact {
    /// Code outside the workspace calls this function: a route handler, a
    /// CLI command, a test, a lifecycle hook.
    Entry {
        node: ModelNode,
        /// `http`, `cli`, `test`, `event`, …
        #[serde(rename = "entryKind")]
        entry_kind: String,
        /// For people: `GET /api/users/[id]`.
        #[serde(default)]
        label: Option<String>,
    },
    /// `from` (a call, or a function) calls the function `to`, in a way the
    /// syntax does not show.
    Invokes { from: ModelNode, to: ModelNode },
    /// The value of `from` becomes the value of `to`.
    Flows { from: ModelNode, to: ModelNode },
    /// One end of a channel. The provider joins producers with consumers.
    Channel(ChannelFact),
    /// Labels on a node: `source:user-input`, `sink:sql`, `pii`, …
    Tag {
        node: ModelNode,
        labels: Vec<String>,
    },
}

/// The facts one model stated about one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelFileFacts {
    pub model: String,
    /// The file the model was run on. Replacing its facts replaces these.
    pub file: PathBuf,
    pub facts: Vec<ModelFact>,
}

/// One end of a channel as the provider sees it after the join.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelReport {
    pub model: String,
    pub side: ChannelSide,
    pub protocol: String,
    /// [`ChannelKey::display`].
    pub key: String,
    pub node: ModelNode,
    /// The key has an unknown part: it was not joined to anything.
    pub open: bool,
    /// Ends on the other side it was joined to.
    pub matches: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(parts: &[&str]) -> ChannelKey {
        ChannelKey {
            protocol: "http".to_string(),
            text: None,
            parts: parts
                .iter()
                .map(|part| match *part {
                    "?" => KeyPart::Unknown { unknown: true },
                    p if p.starts_with("...") => KeyPart::Rest {
                        rest: p[3..].to_string(),
                    },
                    p if p.starts_with(':') => KeyPart::Hole {
                        hole: p[1..].to_string(),
                    },
                    p => KeyPart::Literal(p.to_string()),
                })
                .collect(),
        }
    }

    #[test]
    fn literals_and_holes() {
        let route = key(&["GET", "api", "users", ":id"]);
        let call = key(&["GET", "api", "users", ":userId"]);
        let found = match_keys(&call, &route).unwrap();
        assert!(found.exact && !found.open);
        assert_eq!(
            found.bindings,
            vec![("userId".to_string(), "id".to_string())]
        );

        // A fixed segment fills a parameter; a different method does not match.
        assert!(
            match_keys(&key(&["GET", "api", "users", "me"]), &route)
                .unwrap()
                .exact
        );
        assert!(match_keys(&key(&["POST", "api", "users", ":id"]), &route).is_none());
        assert!(match_keys(&key(&["GET", "api", "users"]), &route).is_none());

        // A varying segment against fixed text: possible, not exact.
        let fixed = key(&["GET", "api", "users", "me"]);
        assert!(!match_keys(&call, &fixed).unwrap().exact);
    }

    #[test]
    fn rest_and_unknown() {
        let catch_all = key(&["GET", "docs", "...slug"]);
        assert!(match_keys(&key(&["GET", "docs", "a", "b"]), &catch_all).is_some());
        assert!(match_keys(&key(&["GET", "docs"]), &catch_all).is_none());

        let unknown = key(&["GET", "?"]);
        let found = match_keys(&unknown, &catch_all).unwrap();
        assert!(found.open && !found.exact);
        assert!(unknown.is_open());
        assert_eq!(catch_all.display(), "http GET/docs/{...slug}");
        assert_eq!(unknown.display(), "http GET/?");
    }

    #[test]
    fn facts_round_trip_as_json() {
        let json = r#"[
            {"kind":"entry","node":{"file":"a.ts","start":1,"end":9},"entryKind":"http","label":"GET /x"},
            {"kind":"channel","side":"consumer","key":{"protocol":"http","parts":["GET","x",{"hole":"id"},{"rest":"r"},{"unknown":true}]},
             "node":{"file":"a.ts","start":1,"end":9},"receives":[{"name":"id","node":{"file":"a.ts","start":2,"end":3}}]},
            {"kind":"tag","node":{"file":"a.ts","start":2,"end":3},"labels":["source:user-input"]}
        ]"#;
        let facts: Vec<ModelFact> = serde_json::from_str(json).unwrap();
        assert_eq!(facts.len(), 3);
        let ModelFact::Channel(channel) = &facts[1] else {
            panic!("not a channel");
        };
        assert_eq!(channel.key.parts.len(), 5);
        assert!(matches!(channel.key.parts[3], KeyPart::Rest { .. }));
        let again: Vec<ModelFact> =
            serde_json::from_str(&serde_json::to_string(&facts).unwrap()).unwrap();
        assert_eq!(facts, again);
    }
}
