//! JSONL worker protocol: one JSON object per line on the reader, one JSON
//! object per line on the writer, strictly request/response in order.
//!
//! ```text
//! -> {"type":"open","protocolVersion":3,"script":"transform.ts","scriptRoot":"/abs/workflow",
//!     "language":"typescript","targetRoot":"/abs/repo","semanticAnalysis":"workspace","input":{...}}
//! <- {"type":"opened","protocolVersion":3,"extensions":[".ts",...],"semanticMode":"workspace"}
//! -> {"type":"index","path":"src/a.ts","content":"..."}
//! <- {"type":"indexed"}
//! -> {"type":"transform","path":"src/a.ts","content":"..."}
//! <- {"type":"transformed","result":{"primary":{...},"secondary":[...],"output":...}}
//! -> {"type":"close"}
//! <- {"type":"closed"}
//! ```
//!
//! Every message denies unknown fields on both sides. An `error` response
//! with `fatal: true` (malformed input, protocol misuse, failed `open`) is the
//! worker's last message; `fatal: false` (a transform or index failure) leaves
//! the session usable. EOF on the reader ends the worker, which is how a
//! killed host never leaves a worker behind.

use std::io::{BufRead, Write};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    session::{JssgSession, SessionConfig, TransformResult},
    SemanticAnalysis, SemanticMode, PROTOCOL_VERSION,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "lowercase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum WorkerRequest {
    Open {
        protocol_version: u32,
        script: String,
        script_root: String,
        language: String,
        target_root: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        semantic_analysis: Option<SemanticAnalysis>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<Value>,
    },
    Index {
        path: String,
        content: String,
    },
    Transform {
        path: String,
        content: String,
    },
    Close {},
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "lowercase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum WorkerResponse {
    Opened {
        protocol_version: u32,
        extensions: Vec<String>,
        semantic_mode: Option<SemanticMode>,
    },
    Indexed {},
    Transformed {
        result: TransformResult,
    },
    Closed {},
    Error {
        message: String,
        fatal: bool,
    },
}

/// Process exit codes of the worker mode.
pub const EXIT_OK: u8 = 0;
pub const EXIT_PROTOCOL: u8 = 3;
pub const EXIT_IO: u8 = 4;

/// Run the worker loop until `close`, EOF, or a fatal error. Blocking; the
/// caller supplies the tokio runtime the sandbox futures run on.
pub fn run_worker<R: BufRead, W: Write>(
    runtime: &tokio::runtime::Handle,
    reader: R,
    mut writer: W,
) -> u8 {
    let mut session: Option<JssgSession> = None;
    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => return EXIT_IO,
        };
        if line.trim().is_empty() {
            continue;
        }
        let request: WorkerRequest = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                return finish(
                    &mut writer,
                    WorkerResponse::Error {
                        message: format!("invalid worker message: {error}"),
                        fatal: true,
                    },
                    EXIT_PROTOCOL,
                );
            }
        };
        let (response, exit) = handle(runtime, &mut session, request);
        if let Some(code) = exit {
            return finish(&mut writer, response, code);
        }
        if write_line(&mut writer, &response).is_err() {
            return EXIT_IO;
        }
    }
    EXIT_OK
}

fn handle(
    runtime: &tokio::runtime::Handle,
    session: &mut Option<JssgSession>,
    request: WorkerRequest,
) -> (WorkerResponse, Option<u8>) {
    let fatal = |message: String| {
        (
            WorkerResponse::Error {
                message,
                fatal: true,
            },
            Some(EXIT_PROTOCOL),
        )
    };
    match request {
        WorkerRequest::Open {
            protocol_version,
            script,
            script_root,
            language,
            target_root,
            semantic_analysis,
            input,
        } => {
            if protocol_version != PROTOCOL_VERSION {
                return fatal(format!(
                    "unsupported protocolVersion {protocol_version} (expected {PROTOCOL_VERSION})"
                ));
            }
            if session.is_some() {
                return fatal("session is already open".to_string());
            }
            let config = SessionConfig {
                script,
                script_root: script_root.into(),
                language,
                target_root: target_root.into(),
                semantic_analysis,
                input,
            };
            match runtime.block_on(JssgSession::open(config)) {
                Ok(opened) => {
                    let info = opened.info().clone();
                    *session = Some(opened);
                    (
                        WorkerResponse::Opened {
                            protocol_version: PROTOCOL_VERSION,
                            extensions: info.extensions,
                            semantic_mode: info.semantic_mode,
                        },
                        None,
                    )
                }
                Err(message) => fatal(message),
            }
        }
        WorkerRequest::Index { path, content } => match session {
            Some(open) => match open.index(&path, &content) {
                Ok(()) => (WorkerResponse::Indexed {}, None),
                Err(message) => (
                    WorkerResponse::Error {
                        message,
                        fatal: false,
                    },
                    None,
                ),
            },
            None => fatal("index before open".to_string()),
        },
        WorkerRequest::Transform { path, content } => match session {
            Some(open) => match runtime.block_on(open.transform(&path, &content)) {
                Ok(result) => (WorkerResponse::Transformed { result }, None),
                Err(message) => (
                    WorkerResponse::Error {
                        message,
                        fatal: false,
                    },
                    None,
                ),
            },
            None => fatal("transform before open".to_string()),
        },
        WorkerRequest::Close {} => {
            *session = None;
            (WorkerResponse::Closed {}, Some(EXIT_OK))
        }
    }
}

fn finish<W: Write>(writer: &mut W, response: WorkerResponse, code: u8) -> u8 {
    if write_line(writer, &response).is_err() {
        return EXIT_IO;
    }
    code
}

fn write_line<W: Write>(writer: &mut W, response: &WorkerResponse) -> std::io::Result<()> {
    let json = serde_json::to_string(response)?;
    writer.write_all(json.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()
}
