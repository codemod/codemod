//! Before a workflow that imports `codemod/assess` runs: is it allowed to
//! send files to the decision model, and is there a key? Both are settled
//! before any step runs, so a run without them fails at once instead of at
//! the first `assess()`.
//!
//! The build knows statically whether the workflow (or a local file it
//! imports) imports `codemod/assess` ([`codemod_workflow::uses_assess`]).
//! The provider and model come from the workflow's `codemod.yaml`, the user
//! config and the environment ([`codemod_workflow::assess::config`]).

use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Result};
use codemod_workflow::assess::config::{Env, Layers};
use codemod_workflow::assess::{DecisionModel, HttpDecisionModels, ModelPreference, Settings};
use console::style;

/// How the person running the workflow is asked.
pub(crate) trait Prompts {
    /// May the workflow send files to the model?
    fn allow(&self, settings: &Settings) -> bool;
    /// A key for this run, or `None` to stop.
    fn key(&self, settings: &Settings) -> Option<String>;
}

pub(crate) struct TerminalPrompts;

impl Prompts for TerminalPrompts {
    fn allow(&self, settings: &Settings) -> bool {
        eprintln!();
        eprintln!(
            "  {} {}",
            style("⚠").yellow().bold(),
            style("AI assessment").yellow().bold(),
        );
        eprintln!(
            "  {}",
            style(format!(
                "This workflow uses codemod/assess: it sends the contents of files, and of the files they import, to {} ({}, model {}).",
                settings.provider.display_name(),
                settings.host(),
                settings.model
            ))
            .dim()
        );
        eprintln!(
            "  {}",
            style("This applies only to the current run; --allow-ai allows it without asking.")
                .dim()
        );
        inquire::Confirm::new("Allow it?")
            .with_default(false)
            .prompt()
            .unwrap_or(false)
    }

    fn key(&self, settings: &Settings) -> Option<String> {
        eprintln!();
        eprintln!(
            "  {}",
            style(format!(
                "{} is not set. Paste a {} API key to use for this run; it is not saved.",
                settings.provider.key_var(),
                settings.provider.display_name()
            ))
            .dim()
        );
        inquire::Password::new(&format!("{} API key:", settings.provider.display_name()))
            .without_confirmation()
            .with_display_mode(inquire::PasswordDisplayMode::Masked)
            .prompt()
            .ok()
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty())
    }
}

/// The decision model for a run of `workflow`: `None` when it does not use
/// `codemod/assess`. Fails, before anything runs, when the run may not send
/// files to the model or has no key for it and nobody can be asked.
pub(crate) fn decision_model(
    workflow: &Path,
    allow_ai: bool,
    interactive: bool,
    prompts: &dyn Prompts,
) -> Result<Option<Arc<dyn DecisionModel>>> {
    if !codemod_workflow::uses_assess(workflow) {
        return Ok(None);
    }
    let env: Env = std::env::vars().collect();
    let layers = Layers::for_workflow(workflow).map_err(anyhow::Error::msg)?;
    decision_model_with(layers, allow_ai, interactive, prompts, env)
}

/// The decision model for a test run of `workflow` (`codemod/testing`):
/// nobody is asked, and a call without a key fails with what to set.
pub(crate) fn test_decision_model(workflow: &Path) -> Option<Arc<dyn DecisionModel>> {
    if !codemod_workflow::uses_assess(workflow) {
        return None;
    }
    let made = Layers::for_workflow(workflow)
        .and_then(|layers| HttpDecisionModels::new(layers, std::env::vars().collect()));
    Some(match made {
        Ok(models) => Arc::new(models),
        Err(error) => Arc::new(codemod_workflow::assess::Unavailable(error)),
    })
}

fn decision_model_with(
    layers: Layers,
    allow_ai: bool,
    interactive: bool,
    prompts: &dyn Prompts,
    env: Env,
) -> Result<Option<Arc<dyn DecisionModel>>> {
    let mut models = HttpDecisionModels::new(layers, env).map_err(anyhow::Error::msg)?;
    let settings = models
        .settings(&ModelPreference::default())
        .map_err(anyhow::Error::msg)?;
    let target = format!(
        "{} ({}, model {})",
        settings.provider.display_name(),
        settings.host(),
        settings.model
    );
    if !allow_ai {
        if !interactive {
            bail!(
                "This workflow uses codemod/assess, which sends the contents of files to {target}. Run it with --allow-ai to allow that."
            );
        }
        if !prompts.allow(&settings) {
            bail!("Stopped: the workflow uses codemod/assess, which was not allowed to send files to {target}.");
        }
    }
    if settings.api_key.is_none() {
        let config = codemod_workflow::assess::config::user_config_path()
            .map(|path| format!(" or \"decisionModel\" in {}", path.display()))
            .unwrap_or_default();
        let missing = format!(
            "This workflow uses codemod/assess, which asks {target}, and no API key is set: set {}, or choose another provider with DECISION_PROVIDER{config}.",
            settings.key_vars()
        );
        if !interactive {
            bail!(missing);
        }
        match prompts.key(&settings) {
            Some(key) => models = models.with_key(settings.provider, key),
            None => bail!(missing),
        }
    }
    Ok(Some(Arc::new(models)))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    struct Scripted {
        allow: bool,
        key: Option<&'static str>,
        asked: Cell<u32>,
    }

    impl Prompts for Scripted {
        fn allow(&self, _: &Settings) -> bool {
            self.asked.set(self.asked.get() + 1);
            self.allow
        }

        fn key(&self, _: &Settings) -> Option<String> {
            self.asked.set(self.asked.get() + 1);
            self.key.map(str::to_string)
        }
    }

    fn scripted(allow: bool, key: Option<&'static str>) -> Scripted {
        Scripted {
            allow,
            key,
            asked: Cell::new(0),
        }
    }

    /// The layers of a package with this `codemod.yaml`, and no user config.
    fn workflow(dir: &Path, yaml: Option<&str>) -> Layers {
        if let Some(yaml) = yaml {
            std::fs::write(dir.join("codemod.yaml"), yaml).unwrap();
        }
        Layers::load(dir, None).unwrap()
    }

    fn env(pairs: &[(&str, &str)]) -> Env {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn a_workflow_without_assess_needs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workflow.ts");
        std::fs::write(&path, "export default async function main() {}\n").unwrap();
        let prompts = scripted(false, None);
        assert!(decision_model(&path, false, false, &prompts)
            .unwrap()
            .is_none());
        assert_eq!(prompts.asked.get(), 0);
    }

    #[test]
    fn without_a_terminal_it_fails_naming_the_flag_and_then_the_key_variable() {
        let dir = tempfile::tempdir().unwrap();
        let path = workflow(dir.path(), None);
        let prompts = scripted(true, Some("never"));
        let error = decision_model_with(path.clone(), false, false, &prompts, env(&[]))
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("--allow-ai"), "{error}");
        assert!(
            error.contains("System One (api.typesafe.ai, model jev-latest)"),
            "{error}"
        );
        let error = decision_model_with(path.clone(), true, false, &prompts, env(&[]))
            .err()
            .unwrap()
            .to_string();
        assert!(
            error.contains("set TYPESAFE_API_KEY (or DECISION_API_KEY)"),
            "{error}"
        );
        assert_eq!(prompts.asked.get(), 0, "nobody was asked");
        assert!(decision_model_with(
            path.clone(),
            true,
            false,
            &prompts,
            env(&[("TYPESAFE_API_KEY", "k")])
        )
        .unwrap()
        .is_some());
    }

    #[test]
    fn the_workflows_provider_decides_which_key_is_needed() {
        let dir = tempfile::tempdir().unwrap();
        let path = workflow(
            dir.path(),
            Some("name: x\ndecision_model:\n  provider: openai\n"),
        );
        let error = decision_model_with(
            path,
            true,
            false,
            &scripted(true, None),
            env(&[("TYPESAFE_API_KEY", "k")]),
        )
        .err()
        .unwrap()
        .to_string();
        assert!(
            error.contains("OpenAI Decisions (api.openai.com, model gpt-6-luna)"),
            "{error}"
        );
        assert!(error.contains("set OPENAI_API_KEY"), "{error}");
    }

    #[test]
    fn with_a_terminal_it_asks_for_permission_and_a_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = workflow(dir.path(), None);
        let denied = scripted(false, Some("key"));
        let error = decision_model_with(path.clone(), false, true, &denied, env(&[]))
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("was not allowed"), "{error}");
        let granted = scripted(true, Some("key"));
        assert!(
            decision_model_with(path.clone(), false, true, &granted, env(&[]))
                .unwrap()
                .is_some()
        );
        assert_eq!(granted.asked.get(), 2);
        let no_key = scripted(true, None);
        let error = decision_model_with(path.clone(), true, true, &no_key, env(&[]))
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("no API key is set"), "{error}");
    }
}
