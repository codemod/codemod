//! The questions of an `assess()` call, as `yesNo`, `choice`, `number` and
//! `list` build them, and the typed answers made of what the model said.
//!
//! Decision APIs take yes/no, choice and score questions. `number` is a
//! score over its integer range; `list` is one yes/no question per option.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::{DecisionAnswer, DecisionQuestion};

/// Most options of a choice; System One takes 255.
const MAX_OPTIONS: usize = 255;
/// Most levels of a number; both APIs take 10.
const MAX_LEVELS: i64 = 10;
/// Most options of a list: each is a question of its own.
const MAX_LIST_OPTIONS: usize = 50;
/// Question ids the engine uses for its own questions start with this.
const RESERVED_PREFIX: &str = "__codemod";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct OptionSpec {
    name: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum Spec {
    YesNo {
        instructions: String,
        #[serde(default)]
        yes: Option<String>,
        #[serde(default)]
        no: Option<String>,
        #[serde(default)]
        threshold: Option<f64>,
    },
    Choice {
        #[serde(default)]
        instructions: Option<String>,
        options: Vec<OptionSpec>,
    },
    Number {
        instructions: String,
        min: f64,
        max: f64,
        #[serde(default)]
        levels: Option<Vec<String>>,
    },
    List {
        instructions: String,
        options: Vec<OptionSpec>,
        #[serde(default)]
        threshold: Option<f64>,
    },
}

/// One question, ready to ask and to read the answer of.
#[derive(Debug, Clone)]
enum Question {
    YesNo {
        threshold: f64,
        asked: DecisionQuestion,
    },
    Choice {
        options: Vec<String>,
        asked: DecisionQuestion,
    },
    Number {
        min: i64,
        max: i64,
        asked: DecisionQuestion,
    },
    /// One yes/no question per option, asked under `ids`.
    List {
        options: Vec<String>,
        ids: Vec<String>,
        threshold: f64,
        asked: Vec<DecisionQuestion>,
    },
}

/// The questions of one call, by id, in the order written.
#[derive(Debug, Clone)]
pub(super) struct Asked {
    questions: Vec<(String, Question)>,
}

impl Asked {
    pub fn read(questions: &Value) -> Result<Self, String> {
        let Value::Object(map) = questions else {
            return Err(
                "assess(): questions must be an object of questions by id, such as { handlesErrors: yesNo(\"...\") }"
                    .into(),
            );
        };
        if map.is_empty() {
            return Err("assess(): questions must have at least one question".into());
        }
        let mut read = Vec::new();
        for (index, (id, spec)) in map.iter().enumerate() {
            let at = |message: String| format!("assess(): question '{id}': {message}");
            if id.trim().is_empty() {
                return Err("assess(): a question id must not be empty".into());
            }
            if id.starts_with(RESERVED_PREFIX)
                || ["__proto__", "constructor", "prototype"].contains(&id.as_str())
            {
                return Err(format!("assess(): '{id}' cannot be a question id"));
            }
            let spec: Spec = serde_json::from_value(spec.clone()).map_err(|e| {
                at(format!(
                    "{e}; build questions with yesNo, choice, number or list from codemod/assess"
                ))
            })?;
            read.push((id.clone(), question(index, id, spec).map_err(at)?));
        }
        Ok(Self { questions: read })
    }

    /// The questions as the decision model is asked them.
    pub fn decision_questions(&self) -> Vec<(String, DecisionQuestion)> {
        let mut out = Vec::new();
        for (id, question) in &self.questions {
            match question {
                Question::YesNo { asked, .. }
                | Question::Choice { asked, .. }
                | Question::Number { asked, .. } => out.push((id.clone(), asked.clone())),
                Question::List { ids, asked, .. } => {
                    out.extend(ids.iter().cloned().zip(asked.iter().cloned()))
                }
            }
        }
        out
    }

    /// `(answers, details)`: each question's typed value, and what the model
    /// said about it (probabilities, confidence).
    pub fn answers(
        &self,
        got: &BTreeMap<String, DecisionAnswer>,
    ) -> Result<(Value, Value), String> {
        let mut answers = Map::new();
        let mut details = Map::new();
        for (id, question) in &self.questions {
            let answer = |id: &str| -> Result<&DecisionAnswer, String> {
                match got.get(id) {
                    None => Err(format!("the decision model did not answer '{id}'")),
                    Some(DecisionAnswer::Refused) => {
                        Err(format!("the decision model declined to answer '{id}'"))
                    }
                    Some(answer) => Ok(answer),
                }
            };
            let mismatch = |expected: &str| {
                format!("the decision model answered '{id}' with something other than {expected}")
            };
            let (value, detail) = match question {
                Question::YesNo { threshold, .. } => {
                    let DecisionAnswer::YesNo { probability } = answer(id)? else {
                        return Err(mismatch("a yes/no probability"));
                    };
                    let probability = probability_of(*probability, id)?;
                    (
                        json!(probability >= *threshold),
                        json!({ "probability": probability }),
                    )
                }
                Question::Choice { options, .. } => {
                    let DecisionAnswer::Choice {
                        choice,
                        probabilities,
                        confidence,
                    } = answer(id)?
                    else {
                        return Err(mismatch("a choice"));
                    };
                    if !options.contains(choice) {
                        return Err(format!(
                            "the decision model chose '{choice}' for '{id}', which is not one of its options"
                        ));
                    }
                    let probabilities: Map<String, Value> = options
                        .iter()
                        .map(|option| {
                            (
                                option.clone(),
                                json!(probabilities.get(option).copied().unwrap_or(0.0)),
                            )
                        })
                        .collect();
                    (
                        json!(choice),
                        json!({ "probabilities": probabilities, "confidence": confidence }),
                    )
                }
                Question::Number { min, max, .. } => {
                    let DecisionAnswer::Score {
                        score,
                        probabilities,
                        confidence,
                    } = answer(id)?
                    else {
                        return Err(mismatch("a score"));
                    };
                    let levels = (max - min) as f64;
                    if !score.is_finite() || *score < -1e-6 || *score > levels + 1e-6 {
                        return Err(format!(
                            "the decision model scored '{id}' {score}, outside its range"
                        ));
                    }
                    let value = *min as f64 + score.clamp(0.0, levels);
                    let by_value: Map<String, Value> = (0..=(max - min))
                        .map(|level| {
                            let p = probabilities.get(&(level as usize)).copied().unwrap_or(0.0);
                            ((min + level).to_string(), json!(p))
                        })
                        .collect();
                    let likeliest = (0..=(max - min))
                        .max_by(|a, b| {
                            let p =
                                |l: &i64| probabilities.get(&(*l as usize)).copied().unwrap_or(0.0);
                            p(a).total_cmp(&p(b)).then(b.cmp(a))
                        })
                        .map(|level| min + level)
                        .unwrap_or(*min);
                    (
                        json!(value),
                        json!({ "probabilities": by_value, "likeliest": likeliest, "confidence": confidence }),
                    )
                }
                Question::List {
                    options,
                    ids,
                    threshold,
                    ..
                } => {
                    let mut chosen = Vec::new();
                    let mut probabilities = Map::new();
                    for (option, asked_as) in options.iter().zip(ids) {
                        let DecisionAnswer::YesNo { probability } = answer(asked_as)? else {
                            return Err(mismatch("yes/no probabilities"));
                        };
                        let probability = probability_of(*probability, id)?;
                        if probability >= *threshold {
                            chosen.push(json!(option));
                        }
                        probabilities.insert(option.clone(), json!(probability));
                    }
                    (
                        Value::Array(chosen),
                        json!({ "probabilities": probabilities }),
                    )
                }
            };
            answers.insert(id.clone(), value);
            details.insert(id.clone(), detail);
        }
        Ok((Value::Object(answers), Value::Object(details)))
    }
}

fn probability_of(p: f64, id: &str) -> Result<f64, String> {
    if p.is_finite() && (-1e-6..=1.0 + 1e-6).contains(&p) {
        Ok(p.clamp(0.0, 1.0))
    } else {
        Err(format!(
            "the decision model gave '{id}' a probability of {p}"
        ))
    }
}

fn threshold_of(threshold: Option<f64>) -> Result<f64, String> {
    match threshold {
        None => Ok(0.5),
        Some(t) if t > 0.0 && t < 1.0 => Ok(t),
        Some(t) => Err(format!("threshold must be between 0 and 1, not {t}")),
    }
}

fn instructions_of(text: &str) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("instructions must not be empty".into());
    }
    Ok(text.to_string())
}

fn options_of(
    options: Vec<OptionSpec>,
    min: usize,
    max: usize,
) -> Result<Vec<(String, Option<String>)>, String> {
    if options.len() < min {
        return Err(format!("needs at least {min} options"));
    }
    if options.len() > max {
        return Err(format!(
            "takes at most {max} options, not {}",
            options.len()
        ));
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for OptionSpec { name, description } in options {
        if name.trim().is_empty() {
            return Err("an option must not be empty".into());
        }
        if !seen.insert(name.clone()) {
            return Err(format!("option '{name}' is given twice"));
        }
        out.push((name, description.filter(|d| !d.trim().is_empty())));
    }
    Ok(out)
}

fn question(index: usize, id: &str, spec: Spec) -> Result<Question, String> {
    Ok(match spec {
        Spec::YesNo {
            instructions,
            yes,
            no,
            threshold,
        } => Question::YesNo {
            threshold: threshold_of(threshold)?,
            asked: DecisionQuestion::YesNo {
                instructions: instructions_of(&instructions)?,
                yes,
                no,
            },
        },
        Spec::Choice {
            instructions,
            options,
        } => {
            let options = options_of(options, 2, MAX_OPTIONS)?;
            let instructions = match instructions {
                Some(text) => instructions_of(&text)?,
                None => format!("Which option fits best for '{id}'?"),
            };
            Question::Choice {
                options: options.iter().map(|(name, _)| name.clone()).collect(),
                asked: DecisionQuestion::Choice {
                    instructions,
                    options,
                },
            }
        }
        Spec::Number {
            instructions,
            min,
            max,
            levels,
        } => {
            if min.fract() != 0.0 || max.fract() != 0.0 || !min.is_finite() || !max.is_finite() {
                return Err(format!(
                    "min and max must be whole numbers, not {min} and {max}"
                ));
            }
            let (min, max) = (min as i64, max as i64);
            let count = max - min + 1;
            if !(2..=MAX_LEVELS).contains(&count) {
                return Err(format!(
                    "a number's range must have 2 to {MAX_LEVELS} whole numbers; {min} to {max} has {count}"
                ));
            }
            if let Some(levels) = &levels {
                if levels.len() as i64 != count {
                    return Err(format!(
                        "levels must describe each number from {min} to {max}: {count} of them, not {}",
                        levels.len()
                    ));
                }
            }
            let levels = (0..count)
                .map(|i| {
                    let label = (min + i).to_string();
                    let description = levels
                        .as_ref()
                        .and_then(|levels| levels.get(i as usize))
                        .filter(|d| !d.trim().is_empty())
                        .cloned();
                    (label, description)
                })
                .collect();
            Question::Number {
                min,
                max,
                asked: DecisionQuestion::Score {
                    instructions: instructions_of(&instructions)?,
                    levels,
                },
            }
        }
        Spec::List {
            instructions,
            options,
            threshold,
        } => {
            let instructions = instructions_of(&instructions)?;
            let options = options_of(options, 1, MAX_LIST_OPTIONS)?;
            let ids = (0..options.len())
                .map(|option| format!("{RESERVED_PREFIX}_list_{index}_{option}"))
                .collect();
            let asked = options
                .iter()
                .map(|(name, description)| DecisionQuestion::YesNo {
                    instructions: match description {
                        Some(description) => {
                            format!(
                                "{instructions}\n\nDoes this one apply: {name} ({description})?"
                            )
                        }
                        None => format!("{instructions}\n\nDoes this one apply: {name}?"),
                    },
                    yes: None,
                    no: None,
                })
                .collect();
            Question::List {
                options: options.into_iter().map(|(name, _)| name).collect(),
                ids,
                threshold: threshold_of(threshold)?,
                asked,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asked(questions: Value) -> Asked {
        Asked::read(&questions).unwrap()
    }

    #[test]
    fn each_kind_becomes_the_questions_decision_apis_take() {
        let asked = asked(json!({
            "ok": { "type": "yesNo", "instructions": "Handles errors?" },
            "smell": { "type": "choice", "options": [{ "name": "swallowed" }, { "name": "none", "description": "No smell" }] },
            "risk": { "type": "number", "instructions": "Risk?", "min": 1, "max": 3 },
            "uses": { "type": "list", "instructions": "Which does it use?", "options": [{ "name": "fetch" }, { "name": "fs" }] },
        }));
        let questions = asked.decision_questions();
        let ids: Vec<&str> = questions.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "ok",
                "smell",
                "risk",
                "__codemod_list_3_0",
                "__codemod_list_3_1"
            ]
        );
        assert_eq!(
            questions[2].1,
            DecisionQuestion::Score {
                instructions: "Risk?".into(),
                levels: vec![("1".into(), None), ("2".into(), None), ("3".into(), None)],
            }
        );
        let DecisionQuestion::Choice { instructions, .. } = &questions[1].1 else {
            panic!()
        };
        assert_eq!(instructions, "Which option fits best for 'smell'?");
    }

    #[test]
    fn answers_are_typed_values_with_their_probabilities_as_details() {
        let asked = asked(json!({
            "ok": { "type": "yesNo", "instructions": "Handles errors?" },
            "smell": { "type": "choice", "options": [{ "name": "swallowed" }, { "name": "none" }] },
            "risk": { "type": "number", "instructions": "Risk?", "min": 1, "max": 3 },
            "uses": { "type": "list", "instructions": "Which?", "options": [{ "name": "fetch" }, { "name": "fs" }], "threshold": 0.7 },
        }));
        let got = BTreeMap::from([
            ("ok".to_string(), DecisionAnswer::YesNo { probability: 0.2 }),
            (
                "smell".to_string(),
                DecisionAnswer::Choice {
                    choice: "swallowed".into(),
                    probabilities: BTreeMap::from([
                        ("swallowed".into(), 0.9),
                        ("none".into(), 0.1),
                    ]),
                    confidence: 0.8,
                },
            ),
            (
                "risk".to_string(),
                DecisionAnswer::Score {
                    score: 1.5,
                    probabilities: BTreeMap::from([(0, 0.1), (1, 0.3), (2, 0.6)]),
                    confidence: 0.5,
                },
            ),
            (
                "__codemod_list_3_0".to_string(),
                DecisionAnswer::YesNo { probability: 0.9 },
            ),
            (
                "__codemod_list_3_1".to_string(),
                DecisionAnswer::YesNo { probability: 0.6 },
            ),
        ]);
        let (answers, details) = asked.answers(&got).unwrap();
        assert_eq!(
            answers,
            json!({ "ok": false, "smell": "swallowed", "risk": 2.5, "uses": ["fetch"] })
        );
        assert_eq!(details["risk"]["likeliest"], json!(3));
        assert_eq!(details["risk"]["probabilities"]["1"], json!(0.1));
        assert_eq!(details["uses"]["probabilities"]["fs"], json!(0.6));
        assert_eq!(details["ok"]["probability"], json!(0.2));
    }

    #[test]
    fn a_choice_outside_the_options_or_a_refusal_is_an_error() {
        let asked = asked(json!({
            "smell": { "type": "choice", "options": [{ "name": "a" }, { "name": "b" }] },
        }));
        let wrong = BTreeMap::from([(
            "smell".to_string(),
            DecisionAnswer::Choice {
                choice: "c".into(),
                probabilities: BTreeMap::new(),
                confidence: 1.0,
            },
        )]);
        assert!(asked
            .answers(&wrong)
            .unwrap_err()
            .contains("not one of its options"));
        let refused = BTreeMap::from([("smell".to_string(), DecisionAnswer::Refused)]);
        assert!(asked.answers(&refused).unwrap_err().contains("declined"));
        assert!(asked
            .answers(&BTreeMap::new())
            .unwrap_err()
            .contains("did not answer"));
    }

    #[test]
    fn bad_questions_say_what_is_wrong() {
        let error = |questions: Value| Asked::read(&questions).unwrap_err();
        assert!(error(json!({})).contains("at least one question"));
        assert!(error(json!([])).contains("object of questions"));
        assert!(
            error(json!({ "__codemod_x": { "type": "yesNo", "instructions": "x" } }))
                .contains("cannot be a question id")
        );
        assert!(
            error(json!({ "a": { "type": "choice", "options": [{ "name": "only" }] } }))
                .contains("at least 2 options")
        );
        assert!(error(
            json!({ "a": { "type": "number", "instructions": "x", "min": 0, "max": 10 } })
        )
        .contains("2 to 10 whole numbers"));
        assert!(
            error(json!({ "a": { "type": "yesNo", "instructions": "x", "threshold": 1.5 } }))
                .contains("threshold")
        );
        assert!(error(json!({ "a": "is it?" })).contains("yesNo, choice, number or list"));
        assert!(
            error(json!({ "a": { "type": "text", "instructions": "x" } }))
                .contains("yesNo, choice, number or list")
        );
    }
}
