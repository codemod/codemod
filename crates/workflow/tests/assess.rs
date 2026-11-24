//! `codemod/assess`: typed questions about files, answered by a fake
//! decision model.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use codemod_workflow::assess::{
    DecisionAnswer, DecisionModel, DecisionQuestion, DecisionRequest, DecisionResponse, TokenUsage,
};
use codemod_workflow::interaction::NonInteractive;
use codemod_workflow::{MemoryStore, Services, StepStatus};
use common::{output, Harness};
use serde_json::{json, Value};

type Respond = dyn Fn(&DecisionRequest) -> BTreeMap<String, DecisionAnswer> + Send + Sync;

/// Answers with `respond`, and keeps every request.
struct Fake {
    requests: Mutex<Vec<DecisionRequest>>,
    respond: Box<Respond>,
}

impl Fake {
    fn new(
        respond: impl Fn(&DecisionRequest) -> BTreeMap<String, DecisionAnswer> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            requests: Mutex::new(Vec::new()),
            respond: Box::new(respond),
        })
    }

    fn requests(&self) -> Vec<DecisionRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl DecisionModel for Fake {
    fn decide(
        &self,
        request: &DecisionRequest,
        _: &AtomicBool,
    ) -> Result<DecisionResponse, String> {
        self.requests.lock().unwrap().push(request.clone());
        Ok(DecisionResponse {
            provider: "fake".into(),
            model: "fake-1".into(),
            answers: (self.respond)(request),
            usage: TokenUsage {
                input_tokens: 100,
                output_tokens: 2,
            },
        })
    }
}

/// Answers every question the same way: yes, the first option, the middle level.
fn plain(request: &DecisionRequest) -> BTreeMap<String, DecisionAnswer> {
    request
        .questions
        .iter()
        .map(|(id, question)| {
            let answer = match question {
                DecisionQuestion::YesNo { instructions, .. } => DecisionAnswer::YesNo {
                    probability: if instructions.contains("fs") {
                        0.1
                    } else {
                        0.9
                    },
                },
                DecisionQuestion::Choice { options, .. } => DecisionAnswer::Choice {
                    choice: options[0].0.clone(),
                    probabilities: options
                        .iter()
                        .enumerate()
                        .map(|(i, (name, _))| {
                            (
                                name.clone(),
                                if i == 0 {
                                    0.8
                                } else {
                                    0.2 / (options.len() - 1) as f64
                                },
                            )
                        })
                        .collect(),
                    confidence: 0.7,
                },
                DecisionQuestion::Score { levels, .. } => DecisionAnswer::Score {
                    score: 1.0,
                    probabilities: (0..levels.len())
                        .map(|l| (l, if l == 1 { 1.0 } else { 0.0 }))
                        .collect(),
                    confidence: 0.9,
                },
            };
            (id.clone(), answer)
        })
        .collect()
}

fn harness(model: Arc<Fake>) -> Harness {
    Harness::with_services(
        Arc::new(MemoryStore::default()),
        Arc::new(NonInteractive { approve_all: false }),
        Services {
            decision: Some(model),
            ..Services::default()
        },
    )
}

fn copy_dir(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            std::fs::create_dir_all(&target).unwrap();
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

const REEXPORTS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../codemod-sandbox/tests/integration/semantic/fixtures/typescript/reexport_definition"
);

const CLASSIFY: &str = r#"
import { step, forEachFile } from "codemod/workflow";
import { assess, yesNo, choice, number, list } from "codemod/assess";
export default async function main() {
  return step("Classify", forEachFile("packages/web/src/*.ts", async (file) => {
    const { answers, details, context, model } = await assess(file, {
      questions: {
        handlesErrors: yesNo("Does this file handle the errors its calls can throw?"),
        smell: choice("Which error-handling smell fits best?", { swallowed: "Errors are caught and dropped", none: null }),
        risk: number("How risky is rewriting it?", { min: 1, max: 3 }),
        uses: list("Which of these does it use?", ["dates", "fs"]),
      },
    });
    return { path: file.path, answers, likeliest: details.risk.likeliest, context, model };
  }));
}
"#;

#[test]
fn a_callback_gets_typed_answers_about_its_file_and_the_files_it_imports() {
    let fake = Fake::new(plain);
    let h = harness(Arc::clone(&fake));
    copy_dir(Path::new(REEXPORTS), h.target.path());
    let report = h.start(CLASSIFY, Value::Null);
    let result = output(&report).unwrap();
    assert_eq!(
        result["outputs"][0],
        json!({
            "path": "packages/web/src/app.ts",
            "answers": { "handlesErrors": true, "smell": "swallowed", "risk": 2, "uses": ["dates"] },
            "likeliest": 2,
            // The definitions behind the barrel, not the barrel.
            "context": [
                "packages/web/src/app.ts",
                "packages/shared/src/dates.ts",
                "packages/shared/src/deep.ts",
                "packages/shared/src/inner.ts",
            ],
            "model": "fake-1",
        })
    );
    assert_eq!(
        result["assessed"],
        json!({ "assessments": 1, "calls": 1, "inputTokens": 100, "outputTokens": 2 })
    );

    let requests = fake.requests();
    assert_eq!(requests.len(), 1);
    let state = &requests[0].state;
    assert!(
        state.contains("The file being assessed is packages/web/src/app.ts"),
        "{state}"
    );
    assert!(
        state.contains("----- packages/shared/src/dates.ts (defines toMoment, viaImport) -----"),
        "{state}"
    );
    assert!(state.contains("export function deepFn()"), "{state}");
    assert!(
        !state.contains("----- packages/shared/src/index.ts"),
        "{state}"
    );
    let ids: Vec<&str> = requests[0]
        .questions
        .iter()
        .map(|(id, _)| id.as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "handlesErrors",
            "smell",
            "risk",
            "__codemod_list_3_0",
            "__codemod_list_3_1"
        ]
    );
    assert!(h.events.take().iter().any(|e| format!("{e:?}")
        .contains("assess: 1 answer, 1 model call, 100 input and 2 output tokens")));
}

#[test]
fn dependents_and_extra_files_are_shown_when_asked_for() {
    let fake = Fake::new(plain);
    let h = harness(Arc::clone(&fake));
    copy_dir(Path::new(REEXPORTS), h.target.path());
    let report = h.start(
        r#"
import { step } from "codemod/workflow";
import { assess, yesNo } from "codemod/assess";
export default async function main() {
  return step("Ask", async () => {
    const { context } = await assess("packages/shared/src/deep.ts", {
      questions: { used: yesNo("Is deepFn used?") },
      context: { imports: false, dependents: true, extra: ["tsconfig.json"] },
    });
    return context;
  });
}
"#,
        Value::Null,
    );
    assert_eq!(
        output(&report).unwrap(),
        json!([
            "packages/shared/src/deep.ts",
            "packages/shared/src/mid.ts",
            "packages/web/src/app.ts",
            "tsconfig.json",
        ])
    );
    let state = &fake.requests()[0].state;
    assert!(
        state.contains("## Files that import packages/shared/src/deep.ts"),
        "{state}"
    );
    assert!(state.contains("## Other files"), "{state}");
}

#[test]
fn the_model_may_ask_for_more_files_a_bounded_number_of_times() {
    // Always asks for the first file it may see.
    let fake = Fake::new(|request| {
        let mut answers = plain(request);
        if let Some((_, DecisionQuestion::Choice { options, .. })) = request
            .questions
            .iter()
            .find(|(id, _)| id == "__codemod_more_context")
        {
            answers.insert(
                "__codemod_more_context".into(),
                DecisionAnswer::Choice {
                    choice: options[1].0.clone(),
                    probabilities: BTreeMap::new(),
                    confidence: 0.9,
                },
            );
        }
        answers
    });
    let h = harness(Arc::clone(&fake));
    copy_dir(Path::new(REEXPORTS), h.target.path());
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
import { assess, yesNo } from "codemod/assess";
export default async function main() {
  return step(forEachFile("packages/shared/src/dates.ts", async (file) => {
    const { context, usage } = await assess(file, {
      questions: { pure: yesNo("Are these functions pure?") },
      context: { imports: false },
      allowMoreContext: 2,
    });
    return { context, usage };
  }));
}
"#,
        Value::Null,
    );
    let result = output(&report).unwrap();
    // Two rounds of asking, then the answer: three calls.
    assert_eq!(
        result["outputs"][0],
        json!({
            "context": [
                "packages/shared/src/dates.ts",
                "packages/shared/src/index.ts",
                "packages/shared/src/local.ts",
            ],
            "usage": { "calls": 3, "inputTokens": 300, "outputTokens": 6 },
        })
    );
    let requests = fake.requests();
    assert_eq!(requests.len(), 3);
    let has_more = |r: &DecisionRequest| {
        r.questions
            .iter()
            .any(|(id, _)| id == "__codemod_more_context")
    };
    assert!(has_more(&requests[0]) && has_more(&requests[1]));
    assert!(!has_more(&requests[2]), "the last call may not ask again");
    assert!(requests[2].state.contains("## Files asked for"));
    assert_eq!(result["assessed"]["calls"], json!(3));
}

#[test]
fn a_resumed_run_does_not_ask_again_for_a_finished_step() {
    let fake = Fake::new(plain);
    let h = harness(Arc::clone(&fake));
    h.write("a.ts", "export const a = 1;\n");
    let workflow = |last: &str| {
        format!(
            r#"
import {{ step, forEachFile }} from "codemod/workflow";
import {{ assess, yesNo }} from "codemod/assess";
export default async function main() {{
  const found = await step("Ask", forEachFile("*.ts", async (file) => {{
    const {{ answers }} = await assess(file, {{ questions: {{ ok: yesNo("Is it fine?") }} }});
    return answers.ok;
  }}));
  await step("Then", () => {{ {last} }});
  return found.outputs;
}}
"#
        )
    };
    let report = h.start(&workflow("throw new Error('not yet');"), Value::Null);
    assert!(output(&report).unwrap_err().contains("not yet"));
    assert_eq!(fake.requests().len(), 1);
    let resumed = h.resume(&report, Some(&workflow("return 1;")));
    assert_eq!(output(&resumed).unwrap(), json!([true]));
    assert_eq!(fake.requests().len(), 1, "the finished step was replayed");
}

#[test]
fn a_dry_run_still_asks() {
    let fake = Fake::new(plain);
    let h = harness(Arc::clone(&fake));
    h.write("a.ts", "export const a = 1;\n");
    let report = h.dry_run(
        r#"
import { step, forEachFile } from "codemod/workflow";
import { assess, yesNo } from "codemod/assess";
export default async function main() {
  return step(forEachFile("*.ts", async (file) => {
    const { answers } = await assess(file, { questions: { ok: yesNo("Is it fine?") } });
    if (answers.ok) file.write("changed");
  }));
}
"#,
    );
    output(&report).unwrap();
    assert_eq!(fake.requests().len(), 1);
    assert_eq!(h.read("a.ts"), "export const a = 1;\n");
}

#[test]
fn assess_rejects_in_the_body_and_without_a_decision_model() {
    let fake = Fake::new(plain);
    let h = harness(Arc::clone(&fake));
    h.write("a.ts", "x");
    // The question builders are data: the body may use them.
    let report = h.start(
        r#"
import { assess, yesNo } from "codemod/assess";
const QUESTIONS = { ok: yesNo("Is it fine?") };
export default async function main() {
  try {
    await assess("a.ts", { questions: QUESTIONS });
    return "asked";
  } catch (error) {
    return String(error);
  }
}
"#,
        Value::Null,
    );
    let message = output(&report).unwrap();
    assert!(
        message
            .as_str()
            .unwrap()
            .contains("works inside a forEachFile callback or a function step"),
        "{message}"
    );
    assert!(fake.requests().is_empty());

    let h = Harness::new();
    h.write("a.ts", "x");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
import { assess, yesNo } from "codemod/assess";
export default async function main() {
  return step(forEachFile("*.ts", (file) => assess(file, { questions: { ok: yesNo("Is it fine?") } })));
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(error.contains("assess() needs a decision model"), "{error}");
}

#[test]
fn bad_questions_fail_the_file_with_the_reason() {
    let fake = Fake::new(plain);
    let h = harness(Arc::clone(&fake));
    h.write("a.ts", "x");
    let report = h.start(
        r#"
import { step, forEachFile } from "codemod/workflow";
import { assess, number } from "codemod/assess";
export default async function main() {
  return step(forEachFile("*.ts", (file) => assess(file, { questions: { n: number("How many?", { min: 0, max: 20 }) } })));
}
"#,
        Value::Null,
    );
    let error = output(&report).unwrap_err();
    assert!(
        error.contains("question 'n': a number's range must have 2 to 10 whole numbers"),
        "{error}"
    );
    assert_eq!(report.steps[0].status, StepStatus::Failed);
    assert!(fake.requests().is_empty());
}

#[test]
fn a_workflow_uses_assess_when_it_or_a_local_file_it_imports_does() {
    let h = Harness::new();
    h.bundle_file(
        "lib/questions.ts",
        "export { yesNo } from \"codemod/assess\";\n",
    );
    h.bundle_file("lib/plain.ts", "export const x = 1;\n");
    let direct = h.bundle.path().join("direct.ts");
    std::fs::write(
        &direct,
        "import { assess } from \"codemod:assess\";\nexport default async function main() {}\n",
    )
    .unwrap();
    let helper = h.bundle.path().join("helper.ts");
    std::fs::write(
        &helper,
        "import { yesNo } from \"./lib/questions.ts\";\nexport default async function main() {}\n",
    )
    .unwrap();
    let none = h.bundle.path().join("none.ts");
    std::fs::write(
        &none,
        "import { x } from \"./lib/plain.ts\";\nexport default async function main() {}\n",
    )
    .unwrap();
    assert!(codemod_workflow::uses_assess(&direct));
    assert!(codemod_workflow::uses_assess(&helper));
    assert!(!codemod_workflow::uses_assess(&none));
}
