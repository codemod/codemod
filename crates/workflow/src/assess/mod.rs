//! `codemod/assess`: typed questions about a file, answered by a decision
//! model (TypeSafe's System One API by default, or OpenAI's Decisions API).
//!
//! A `forEachFile` callback or function step calls `assess(file, { questions,
//! context })`. The isolate (or the Node.js host) sends the request here
//! ([`ask`]), which:
//!
//! 1. reads the questions ([`questions`]) and turns them into the yes/no,
//!    choice and score questions decision APIs take;
//! 2. assembles the file's context deterministically ([`context`]): the files
//!    it imports, resolved through re-exports, the files that import it, and
//!    extra files, so the model does not spend tokens finding them;
//! 3. asks the run's [`DecisionModel`], letting the model ask for one more
//!    file at a time when `allowMoreContext` allows it;
//! 4. returns typed answers, the files shown, and token usage, which is also
//!    added up per step.
//!
//! The model is the run's ([`crate::Services::decision`]): the CLI sets up
//! [`HttpDecisionModels`] from the call, the workflow's `codemod.yaml`, the
//! user config and the environment ([`config`]); tests give a fake.

pub mod config;
mod context;
mod http;
mod questions;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::run::RunContext;

pub use config::{ModelPreference, Provider, Settings};
pub use http::HttpDecisionModels;

/// A question as decision APIs take it.
#[derive(Debug, Clone, PartialEq)]
pub enum DecisionQuestion {
    /// Answered with the probability of yes.
    YesNo {
        instructions: String,
        yes: Option<String>,
        no: Option<String>,
    },
    /// One option of several: `(name, description)`.
    Choice {
        instructions: String,
        options: Vec<(String, Option<String>)>,
    },
    /// Ordered levels, from index 0: `(label, description)`.
    Score {
        instructions: String,
        levels: Vec<(String, Option<String>)>,
    },
}

/// What a decision model is asked: questions about `state`, the file and its
/// context as text.
#[derive(Debug, Clone)]
pub struct DecisionRequest {
    /// The provider the call names, if it names one.
    pub provider: Option<String>,
    /// The model the call names, if it names one.
    pub model: Option<String>,
    pub state: String,
    /// In the order they are asked.
    pub questions: Vec<(String, DecisionQuestion)>,
    /// How long one attempt may take.
    pub timeout: Duration,
}

/// One answer.
#[derive(Debug, Clone, PartialEq)]
pub enum DecisionAnswer {
    YesNo {
        probability: f64,
    },
    Choice {
        choice: String,
        probabilities: BTreeMap<String, f64>,
        confidence: f64,
    },
    Score {
        /// Probability-weighted level index; may fall between levels.
        score: f64,
        /// By level index.
        probabilities: BTreeMap<usize, f64>,
        confidence: f64,
    },
    /// The model declined to answer (OpenAI's `refusal`).
    Refused,
}

/// Token counts the provider reported.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone)]
pub struct DecisionResponse {
    /// The provider that answered (`systemone`, `openai`), for the result.
    pub provider: String,
    /// The model that answered, as the provider names it (`jev-1.13.0`).
    pub model: String,
    pub answers: BTreeMap<String, DecisionAnswer>,
    pub usage: TokenUsage,
}

/// Answers questions about a file. Called from blocking threads; `cancel`
/// is set when the step must stop.
pub trait DecisionModel: Send + Sync {
    fn decide(
        &self,
        request: &DecisionRequest,
        cancel: &AtomicBool,
    ) -> Result<DecisionResponse, String>;
}

/// What a step's `assess()` calls used.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepUsage {
    /// `assess()` calls that were answered.
    pub assessments: u64,
    /// Requests to the decision model (more than `assessments` when the model
    /// asked for more context).
    pub calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl StepUsage {
    /// One line for the step's log.
    pub fn describe(&self) -> String {
        format!(
            "assess: {} {}, {} model {}, {} input and {} output tokens",
            self.assessments,
            plural(self.assessments, "answer", "answers"),
            self.calls,
            plural(self.calls, "call", "calls"),
            self.input_tokens,
            self.output_tokens
        )
    }
}

fn plural<'a>(n: u64, one: &'a str, many: &'a str) -> &'a str {
    if n == 1 {
        one
    } else {
        many
    }
}

/// A run's assessments: usage by step, and what context assembly can reuse.
#[derive(Default)]
pub(crate) struct RunAssess {
    usage: Mutex<HashMap<u64, StepUsage>>,
    /// The target's import graph, by step: built when a step first asks for
    /// dependents. Files may change between steps, not within one.
    dependents: Mutex<HashMap<u64, Arc<context::DependentsIndex>>>,
}

impl RunAssess {
    fn add(&self, position: u64, usage: TokenUsage, calls: u64, answered: bool) {
        let mut all = self.usage.lock().unwrap_or_else(|e| e.into_inner());
        let step = all.entry(position).or_default();
        step.calls += calls;
        step.input_tokens += usage.input_tokens;
        step.output_tokens += usage.output_tokens;
        if answered {
            step.assessments += 1;
        }
    }

    /// What the step at `position` used, which is then forgotten; `None`
    /// when it made no calls.
    pub fn take(&self, position: u64) -> Option<StepUsage> {
        let usage = self
            .usage
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&position)?;
        (usage.calls > 0).then_some(usage)
    }

    fn dependents(
        &self,
        position: u64,
        build: impl FnOnce() -> context::DependentsIndex,
    ) -> Arc<context::DependentsIndex> {
        // Built under the lock: workers of one step wait for one index.
        let mut all = self.dependents.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(all.entry(position).or_insert_with(|| Arc::new(build())))
    }

    /// Forget the step's import graph once it is done.
    pub fn finish_step(&self, position: u64) {
        self.dependents
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&position);
    }
}

/// At most `limit` holders at a time; waiting stops when `cancel` is set.
pub(crate) struct Limiter {
    limit: usize,
    held: Mutex<usize>,
    freed: Condvar,
}

pub(crate) struct Permit<'a>(&'a Limiter);

impl Limiter {
    pub fn new(limit: usize) -> Self {
        Self {
            limit: limit.max(1),
            held: Mutex::new(0),
            freed: Condvar::new(),
        }
    }

    pub fn acquire(&self, cancel: &AtomicBool) -> Result<Permit<'_>, String> {
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        while *held >= self.limit {
            if cancel.load(Ordering::SeqCst) {
                return Err(CANCELED.to_string());
            }
            held = self
                .freed
                .wait_timeout(held, Duration::from_millis(50))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        *held += 1;
        Ok(Permit(self))
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut held = self.0.held.lock().unwrap_or_else(|e| e.into_inner());
        *held -= 1;
        self.0.freed.notify_one();
    }
}

pub(crate) const CANCELED: &str = "the assessment was canceled";

/// Why a run has no decision model.
const NO_MODEL: &str = "assess() needs a decision model, and this run has none: the CLI sets one up when the workflow, or a file it imports, imports codemod/assess (see `codemod workflow run --help` for --allow-ai)";

/// Default time one request may take.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Most rounds of `allowMoreContext`.
const MAX_MORE_CONTEXT: u32 = 5;

/// The question that lets the model ask for another file.
const MORE_CONTEXT: &str = "__codemod_more_context";
const ENOUGH: &str = "enough";

/// An `assess()` call, as the isolate or the Node.js host sends it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AskRequest {
    /// Relative to the target, with `/`.
    path: String,
    /// The file's text, when the caller has it (a `forEachFile` file);
    /// otherwise it is read.
    #[serde(default)]
    text: Option<String>,
    questions: Value,
    #[serde(default)]
    context: context::ContextOptions,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    allow_more_context: Option<u32>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

pub(crate) fn install_notice(ctx: &rquickjs::Ctx<'_>) -> rquickjs::Result<()> {
    fn notice(ctx: rquickjs::Ctx<'_>) {
        codemod_sandbox::sandbox::runtime_module::experimental(&ctx, "codemod/assess");
    }
    ctx.globals().set(
        "__codemodExperimentalAssess",
        rquickjs::Function::new(ctx.clone(), notice)?,
    )
}

/// Answer one `assess()` call for the step at `position`. Returns the
/// assessment as JSON: `{ answers, details, context, provider, model, usage }`.
pub(crate) fn ask(
    run: &RunContext,
    position: u64,
    cancel: &AtomicBool,
    request: &str,
) -> Result<String, String> {
    run.experimental("codemod/assess");
    let request: AskRequest =
        serde_json::from_str(request).map_err(|e| format!("assess(): {e}"))?;
    let asked = questions::Asked::read(&request.questions)?;
    let rounds = request.allow_more_context.unwrap_or(0);
    if rounds > MAX_MORE_CONTEXT {
        return Err(format!(
            "assess(): allowMoreContext is at most {MAX_MORE_CONTEXT}, not {rounds}"
        ));
    }
    let timeout = match request.timeout_ms {
        Some(0) => return Err("assess(): timeout must be more than 0".into()),
        Some(ms) => Duration::from_millis(ms),
        None => DEFAULT_TIMEOUT,
    };
    let model = run.services.decision.clone().ok_or(NO_MODEL)?;
    let mut assembled = context::assemble(
        run,
        position,
        &request.path,
        request.text,
        &request.context,
        rounds > 0,
    )?;

    let mut usage = TokenUsage::default();
    let mut calls = 0;
    let mut requested = 0;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(CANCELED.into());
        }
        let candidates = if requested < rounds {
            assembled.candidates()
        } else {
            Vec::new()
        };
        let mut questions = asked.decision_questions();
        if !candidates.is_empty() {
            questions.push((MORE_CONTEXT.to_string(), more_context(&candidates)));
        }
        let decided = model.decide(
            &DecisionRequest {
                provider: request.provider.clone(),
                model: request.model.clone(),
                state: assembled.state(),
                questions,
                timeout,
            },
            cancel,
        );
        let response = match decided {
            Ok(response) => response,
            Err(error) => {
                run.assess.add(position, usage, calls, false);
                return Err(error);
            }
        };
        calls += 1;
        usage.input_tokens += response.usage.input_tokens;
        usage.output_tokens += response.usage.output_tokens;
        if let Some(DecisionAnswer::Choice { choice, .. }) = response.answers.get(MORE_CONTEXT) {
            if choice != ENOUGH && candidates.contains(choice) {
                assembled.request(choice)?;
                requested += 1;
                continue;
            }
        }
        let read = asked.answers(&response.answers);
        run.assess.add(position, usage, calls, read.is_ok());
        let (answers, details) = read?;
        return Ok(json!({
            "answers": answers,
            "details": details,
            "context": assembled.files(),
            "provider": response.provider,
            "model": response.model,
            "usage": {
                "calls": calls,
                "inputTokens": usage.input_tokens,
                "outputTokens": usage.output_tokens,
            },
        })
        .to_string());
    }
}

fn more_context(candidates: &[String]) -> DecisionQuestion {
    let mut options = vec![(
        ENOUGH.to_string(),
        Some("The files shown are enough to answer the other questions.".to_string()),
    )];
    options.extend(candidates.iter().map(|path| {
        (
            path.clone(),
            Some(format!(
                "The other questions cannot be answered well without seeing {path}."
            )),
        )
    }));
    DecisionQuestion::Choice {
        instructions: "Is the context shown enough to answer the other questions, or which one file would you need to see?".to_string(),
        options,
    }
}

/// Install `__assessHost` in an isolate of the step at `position`:
///   ask(requestJson): Promise<string>  `{ ok: true, value }` or `{ ok: false, error }`
pub(crate) fn install(
    ctx: &rquickjs::Ctx<'_>,
    run: Arc<RunContext>,
    position: u64,
    cancel: Arc<AtomicBool>,
) -> rquickjs::Result<()> {
    use rquickjs::prelude::Async;
    use rquickjs::{Function, Object};

    let host = Object::new(ctx.clone())?;
    host.set(
        "ask",
        Function::new(
            ctx.clone(),
            Async(move |request: String| {
                let run = Arc::clone(&run);
                let cancel = Arc::clone(&cancel);
                async move {
                    let answer =
                        tokio::task::spawn_blocking(move || ask(&run, position, &cancel, &request))
                            .await
                            .unwrap_or_else(|e| Err(format!("the assessment stopped: {e}")));
                    let answer = match answer {
                        Ok(value) => format!(r#"{{"ok":true,"value":{value}}}"#),
                        Err(error) => json!({ "ok": false, "error": error }).to_string(),
                    };
                    rquickjs::Result::Ok(answer)
                }
            }),
        )?,
    )?;
    ctx.globals().set("__assessHost", host)
}

/// What an isolate needs to answer `assess()` for its step.
#[derive(Clone)]
pub(crate) struct AssessHandle {
    pub run: Arc<RunContext>,
    pub position: u64,
    pub cancel: Arc<AtomicBool>,
}

impl AssessHandle {
    pub fn new(task: &crate::tasks::TaskContext<'_>) -> Self {
        Self {
            run: Arc::clone(task.run),
            position: task.position,
            cancel: Arc::clone(&task.cancel),
        }
    }
}

/// A model that fails every call with `reason`: for runs that may not ask.
pub struct Unavailable(pub String);

impl DecisionModel for Unavailable {
    fn decide(&self, _: &DecisionRequest, _: &AtomicBool) -> Result<DecisionResponse, String> {
        Err(self.0.clone())
    }
}
