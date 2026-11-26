//! The setup screen's state: what runs, where, with which agent and model.
//! Pure: keys in, choices out.

use std::collections::HashMap;
use std::path::PathBuf;

use codemod_workflow::params::DeclaredParam;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;

use crate::user_settings::DecisionModel;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamField {
    pub name: String,
    pub value: String,
    pub type_text: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentOption {
    /// `None` asks when a step needs an agent.
    pub id: Option<String>,
    pub label: String,
    pub installed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Target,
    DryRun,
    Param(usize),
    Agent,
    Model,
    Start,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupAction {
    None,
    Start,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupState {
    pub title: String,
    pub workflow: PathBuf,
    pub target: String,
    pub dry_run: bool,
    /// The run must be a dry run (a pro codemod).
    pub dry_run_locked: bool,
    pub params: Vec<ParamField>,
    pub agents: Vec<AgentOption>,
    pub agent: usize,
    pub provider: String,
    pub models: Vec<String>,
    pub model: usize,
    /// The provider's model list is being fetched.
    pub fetching_models: bool,
    pub focus: usize,
    /// The text being typed into the focused field.
    pub editing: Option<String>,
    pub error: Option<String>,
}

/// What the setup screen starts from.
pub struct SetupInput {
    pub title: String,
    pub workflow: PathBuf,
    pub target: PathBuf,
    pub params: HashMap<String, Value>,
    pub declared: Vec<DeclaredParam>,
    pub dry_run: bool,
    pub dry_run_locked: bool,
    /// Installed coding agents: (id, label).
    pub installed_agents: Vec<(String, String)>,
    pub agent: Option<String>,
    pub decision_model: DecisionModel,
    pub known_models: Vec<String>,
}

/// What the person chose.
#[derive(Debug, Clone, PartialEq)]
pub struct SetupChoices {
    pub target: PathBuf,
    pub params: HashMap<String, Value>,
    pub dry_run: bool,
    pub agent: Option<String>,
    pub decision_model: DecisionModel,
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

impl SetupState {
    pub fn new(input: SetupInput) -> Self {
        // Declared parameters first, in declaration order, then any others
        // given on the command line, by name.
        let mut params: Vec<ParamField> = input
            .declared
            .iter()
            .map(|declared| ParamField {
                name: declared.name.clone(),
                value: input
                    .params
                    .get(&declared.name)
                    .map(value_text)
                    .unwrap_or_default(),
                type_text: Some(declared.type_text.clone()).filter(|t| !t.is_empty()),
                description: declared.description.clone(),
            })
            .collect();
        let mut extra: Vec<(&String, &Value)> = input
            .params
            .iter()
            .filter(|(name, _)| !input.declared.iter().any(|d| &d.name == *name))
            .collect();
        extra.sort_by(|a, b| a.0.cmp(b.0));
        params.extend(extra.into_iter().map(|(name, value)| ParamField {
            name: name.clone(),
            value: value_text(value),
            type_text: None,
            description: None,
        }));

        let mut agents = vec![AgentOption {
            id: None,
            label: "Ask when a step needs one".into(),
            installed: true,
        }];
        agents.extend(
            input
                .installed_agents
                .iter()
                .map(|(id, label)| AgentOption {
                    id: Some(id.clone()),
                    label: label.clone(),
                    installed: true,
                }),
        );
        let agent = match &input.agent {
            None => 0,
            Some(wanted) => {
                let wanted_id = codemod_agents::find(wanted)
                    .map(|a| a.id().to_string())
                    .unwrap_or_else(|| wanted.clone());
                match agents
                    .iter()
                    .position(|a| a.id.as_deref() == Some(wanted_id.as_str()))
                {
                    Some(index) => index,
                    None => {
                        let label = codemod_agents::find(&wanted_id)
                            .map(|a| a.label().to_string())
                            .unwrap_or_else(|| wanted_id.clone());
                        agents.push(AgentOption {
                            id: Some(wanted_id),
                            label,
                            installed: false,
                        });
                        agents.len() - 1
                    }
                }
            }
        };

        let mut state = Self {
            title: input.title,
            workflow: input.workflow,
            target: input.target.display().to_string(),
            dry_run: input.dry_run || input.dry_run_locked,
            dry_run_locked: input.dry_run_locked,
            params,
            agents,
            agent,
            provider: input.decision_model.provider.clone(),
            models: Vec::new(),
            model: 0,
            fetching_models: false,
            focus: 0,
            editing: None,
            error: None,
        };
        state.set_models(input.known_models, &input.decision_model.model);
        state.focus = state.fields().len() - 1;
        state
    }

    /// Replace the model list, keeping `current` chosen (and listed).
    pub fn set_models(&mut self, mut models: Vec<String>, current: &str) {
        if !models.iter().any(|m| m == current) {
            models.insert(0, current.to_string());
        }
        self.model = models.iter().position(|m| m == current).unwrap_or(0);
        self.models = models;
    }

    pub fn model_name(&self) -> &str {
        self.models
            .get(self.model)
            .map(String::as_str)
            .unwrap_or("")
    }

    pub fn fields(&self) -> Vec<Field> {
        let mut fields = vec![Field::Target, Field::DryRun];
        fields.extend((0..self.params.len()).map(Field::Param));
        fields.extend([Field::Agent, Field::Model, Field::Start]);
        fields
    }

    pub fn focused(&self) -> Field {
        let fields = self.fields();
        fields[self.focus.min(fields.len() - 1)]
    }

    fn text_of(&self, field: Field) -> Option<String> {
        match field {
            Field::Target => Some(self.target.clone()),
            Field::Param(i) => Some(self.params[i].value.clone()),
            Field::Model => Some(format!("{}/{}", self.provider, self.model_name())),
            _ => None,
        }
    }

    fn commit_text(&mut self, field: Field, text: String) {
        match field {
            Field::Target => self.target = text.trim().to_string(),
            Field::Param(i) => self.params[i].value = text,
            Field::Model => match DecisionModel::parse(&text) {
                Ok(model) => {
                    if model.provider != self.provider {
                        self.provider = model.provider.clone();
                        self.models = crate::user_settings::known_models(&model.provider);
                    }
                    let models = self.models.clone();
                    self.set_models(models, &model.model);
                }
                Err(error) => self.error = Some(error.to_string()),
            },
            _ => {}
        }
    }

    fn cycle(&mut self, field: Field, forward: bool) {
        let step = |index: usize, len: usize| {
            if len == 0 {
                0
            } else if forward {
                (index + 1) % len
            } else {
                (index + len - 1) % len
            }
        };
        match field {
            Field::DryRun if !self.dry_run_locked => self.dry_run = !self.dry_run,
            Field::Agent => self.agent = step(self.agent, self.agents.len()),
            Field::Model => self.model = step(self.model, self.models.len()),
            _ => {}
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> SetupAction {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return SetupAction::Quit;
        }
        let field = self.focused();
        if let Some(mut text) = self.editing.take() {
            match key.code {
                KeyCode::Enter => self.commit_text(field, text),
                KeyCode::Esc => {}
                KeyCode::Backspace => {
                    text.pop();
                    self.editing = Some(text);
                }
                KeyCode::Char(c) => {
                    text.push(c);
                    self.editing = Some(text);
                }
                _ => self.editing = Some(text),
            }
            return SetupAction::None;
        }
        self.error = None;
        let last = self.fields().len() - 1;
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return SetupAction::Quit,
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                self.focus = self.focus.saturating_sub(1)
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                self.focus = (self.focus + 1).min(last)
            }
            KeyCode::Left | KeyCode::Char('h') => self.cycle(field, false),
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(' ') => self.cycle(field, true),
            KeyCode::Char('e') => self.editing = self.text_of(field),
            KeyCode::Char('s') => return self.start(),
            KeyCode::Enter => match field {
                Field::Start => return self.start(),
                Field::Target | Field::Param(_) | Field::Model => {
                    self.editing = self.text_of(field)
                }
                other => self.cycle(other, true),
            },
            _ => {}
        }
        SetupAction::None
    }

    fn start(&mut self) -> SetupAction {
        match self.choices() {
            Ok(_) => SetupAction::Start,
            Err(error) => {
                self.error = Some(error);
                SetupAction::None
            }
        }
    }

    /// The choices made, or why they cannot start a run.
    pub fn choices(&self) -> Result<SetupChoices, String> {
        let target = PathBuf::from(&self.target);
        if !target.is_dir() {
            return Err(format!("target '{}' is not a directory", self.target));
        }
        let given: Vec<String> = self
            .params
            .iter()
            .filter(|p| !p.value.is_empty())
            .map(|p| format!("{}={}", p.name, p.value))
            .collect();
        let params = butterflow_core::utils::parse_params_keeping_text(&given, |key| {
            self.params.iter().any(|p| {
                p.name == key
                    && p.type_text
                        .as_deref()
                        .is_some_and(codemod_workflow::params::is_string_type)
            })
        })
        .map_err(|error| error.to_string())?;
        Ok(SetupChoices {
            target,
            params,
            dry_run: self.dry_run,
            agent: self.agents.get(self.agent).and_then(|a| a.id.clone()),
            decision_model: DecisionModel {
                provider: self.provider.clone(),
                model: self.model_name().to_string(),
            },
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    pub(crate) fn input(target: PathBuf) -> SetupInput {
        SetupInput {
            title: "moment-to-dayjs".into(),
            workflow: PathBuf::from("workflows/moment-to-dayjs.ts"),
            target,
            params: HashMap::from([
                ("package".to_string(), json!("api")),
                ("extra".to_string(), json!(3)),
            ]),
            declared: vec![
                DeclaredParam {
                    name: "package".into(),
                    type_text: "string".into(),
                    optional: true,
                    description: Some("Only migrate this package.".into()),
                    default: None,
                },
                DeclaredParam {
                    name: "agent".into(),
                    type_text: "boolean | string".into(),
                    optional: true,
                    description: None,
                    default: None,
                },
            ],
            dry_run: false,
            dry_run_locked: false,
            installed_agents: vec![
                ("claude-code".into(), "Claude Code".into()),
                ("codex".into(), "Codex".into()),
            ],
            agent: Some("codex".into()),
            decision_model: DecisionModel::default(),
            known_models: vec!["jev-latest".into(), "jev-preview".into()],
        }
    }

    fn focus(state: &mut SetupState, field: Field) {
        state.focus = state.fields().iter().position(|f| *f == field).unwrap();
    }

    #[test]
    fn starts_from_the_flags_and_saved_settings() {
        let dir = tempfile::tempdir().unwrap();
        let state = SetupState::new(input(dir.path().to_path_buf()));
        assert_eq!(
            state
                .params
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            vec!["package", "agent", "extra"]
        );
        assert_eq!(state.params[0].value, "api");
        assert_eq!(state.params[2].value, "3");
        assert_eq!(state.agents[state.agent].id.as_deref(), Some("codex"));
        assert_eq!(state.model_name(), "jev-latest");
        assert_eq!(state.focused(), Field::Start);

        let choices = state.choices().unwrap();
        assert_eq!(choices.params.get("extra"), Some(&json!(3)));
        assert_eq!(choices.params.get("package"), Some(&json!("api")));
        assert!(!choices.params.contains_key("agent"));
        assert_eq!(choices.agent.as_deref(), Some("codex"));
        assert_eq!(choices.decision_model.id(), "systemone/jev-latest");
    }

    #[test]
    fn a_declared_string_param_stays_text() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = SetupState::new(input(dir.path().to_path_buf()));
        state.params[0].value = "4".into();
        state.params[1].value = "true".into();
        let choices = state.choices().unwrap();
        assert_eq!(choices.params.get("package"), Some(&json!("4")));
        // `boolean | string` may be either, so it is read as JSON.
        assert_eq!(choices.params.get("agent"), Some(&json!(true)));
    }

    #[test]
    fn a_saved_agent_that_is_not_installed_stays_chosen() {
        let dir = tempfile::tempdir().unwrap();
        let mut input = input(dir.path().to_path_buf());
        input.agent = Some("gemini".into());
        let state = SetupState::new(input);
        let chosen = &state.agents[state.agent];
        assert_eq!(chosen.id.as_deref(), Some("gemini"));
        assert!(!chosen.installed);
    }

    #[test]
    fn keys_edit_toggle_and_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = SetupState::new(input(dir.path().to_path_buf()));

        focus(&mut state, Field::Param(1));
        state.key(key(KeyCode::Enter));
        for c in "true".chars() {
            state.key(key(KeyCode::Char(c)));
        }
        state.key(key(KeyCode::Enter));
        assert_eq!(state.params[1].value, "true");

        // Esc while editing keeps the old value.
        focus(&mut state, Field::Param(0));
        state.key(key(KeyCode::Enter));
        state.key(key(KeyCode::Backspace));
        state.key(key(KeyCode::Esc));
        assert_eq!(state.params[0].value, "api");

        focus(&mut state, Field::DryRun);
        state.key(key(KeyCode::Char(' ')));
        assert!(state.dry_run);

        focus(&mut state, Field::Agent);
        state.key(key(KeyCode::Left));
        state.key(key(KeyCode::Left));
        assert_eq!(state.agents[state.agent].id, None);

        focus(&mut state, Field::Model);
        state.key(key(KeyCode::Right));
        assert_eq!(state.model_name(), "jev-preview");

        let choices = state.choices().unwrap();
        assert_eq!(choices.params.get("agent"), Some(&json!(true)));
        assert!(choices.dry_run);
        assert_eq!(choices.agent, None);
        assert_eq!(choices.decision_model.model, "jev-preview");

        focus(&mut state, Field::Start);
        assert_eq!(state.key(key(KeyCode::Enter)), SetupAction::Start);
    }

    #[test]
    fn a_typed_model_may_name_another_provider() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = SetupState::new(input(dir.path().to_path_buf()));
        focus(&mut state, Field::Model);
        state.key(key(KeyCode::Enter));
        state.editing = Some(String::new());
        for c in "ollama/nimble".chars() {
            state.key(key(KeyCode::Char(c)));
        }
        state.key(key(KeyCode::Enter));
        assert_eq!(
            state.choices().unwrap().decision_model.id(),
            "ollama/nimble"
        );
    }

    #[test]
    fn a_locked_dry_run_stays_on() {
        let dir = tempfile::tempdir().unwrap();
        let mut input = input(dir.path().to_path_buf());
        input.dry_run_locked = true;
        let mut state = SetupState::new(input);
        focus(&mut state, Field::DryRun);
        state.key(key(KeyCode::Enter));
        assert!(state.dry_run);
    }

    #[test]
    fn a_missing_target_does_not_start() {
        let mut state = SetupState::new(input(PathBuf::from("/no/such/dir/anywhere")));
        assert_eq!(state.key(key(KeyCode::Enter)), SetupAction::None);
        assert!(state.error.as_deref().unwrap().contains("not a directory"));
        assert_eq!(state.key(key(KeyCode::Char('q'))), SetupAction::Quit);
    }
}
