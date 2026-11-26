//! What a run comes to, as its output is read.
//!
//! A transcript is open until the agent makes its final report
//! ([`Transcript::succeeded`] or [`Transcript::failed`]); the first one
//! stands, and the runner reads no more output into it after that. Agents
//! that make no final report leave it open, and the last text they wrote is
//! the answer.

use std::time::Duration;

use crate::event::{AgentEvent, Usage};
use crate::run::Outcome;

#[derive(Debug, Default)]
pub struct Transcript {
    session_id: Option<String>,
    model: Option<String>,
    usage: Usage,
    usage_is_total: bool,
    /// The last text the agent wrote, and whether more text continues it.
    last_text: String,
    text_open: bool,
    /// The first error the agent reported while it worked.
    error: Option<String>,
    /// The agent's final report: its answer if it gave one, or its failure.
    end: Option<Result<Option<String>, String>>,
}

impl Transcript {
    pub(crate) fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub(crate) fn session(&mut self, id: &str) {
        self.session_id.get_or_insert_with(|| id.to_string());
    }

    pub(crate) fn model(&mut self, model: &str) {
        self.model.get_or_insert_with(|| model.to_string());
    }

    /// Usage for one turn; turns add up.
    pub(crate) fn turn_usage(&mut self, usage: Usage) {
        if !self.usage_is_total {
            self.usage.add(&usage);
        }
    }

    /// Usage for the whole run; replaces what turns added up to.
    pub(crate) fn total_usage(&mut self, usage: Usage) {
        self.usage = usage;
        self.usage_is_total = true;
    }

    /// The agent reported an error while it worked. Agents carry on after
    /// some errors, so this fails the run only when no final report says the
    /// run succeeded.
    pub(crate) fn error(&mut self, message: &str) {
        self.error.get_or_insert_with(|| message.to_string());
    }

    /// The agent's final report says it succeeded, with this answer when it
    /// gives one. Errors reported before it did not stop the agent.
    pub(crate) fn succeeded(&mut self, answer: Option<&str>) {
        let answer = answer.filter(|answer| !answer.trim().is_empty());
        self.end
            .get_or_insert_with(|| Ok(answer.map(str::to_string)));
    }

    /// The agent's final report says it failed.
    pub(crate) fn failed(&mut self, message: &str) {
        self.end.get_or_insert_with(|| Err(message.to_string()));
    }

    /// Whether the agent has made its final report.
    pub(crate) fn ended(&self) -> bool {
        self.end.is_some()
    }

    /// Note an event of the run: the last text the agent writes is its
    /// answer when it reports none, and a tool call or result ends a text.
    pub(crate) fn observe(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::Text { text, delta } => {
                if *delta && self.text_open {
                    self.last_text.push_str(text);
                } else {
                    self.last_text = text.clone();
                }
                self.text_open = true;
            }
            AgentEvent::ToolCall { .. } | AgentEvent::ToolResult { .. } => self.text_open = false,
            _ => {}
        }
    }

    /// What was collected, and the failure the agent reported, if any: its
    /// final one, or with no final report an error it reported on the way.
    /// The first error reported is the message.
    pub(crate) fn into_outcome(
        self,
        agent: &'static str,
        duration: Duration,
    ) -> (Outcome, Option<String>) {
        let (answer, failure) = match self.end {
            Some(Ok(answer)) => (answer, None),
            Some(Err(message)) => (None, Some(self.error.unwrap_or(message))),
            None => (None, self.error),
        };
        let result = answer.or_else(|| {
            let text = self.last_text.trim();
            (!text.is_empty()).then(|| text.to_string())
        });
        let outcome = Outcome {
            agent,
            result,
            session_id: self.session_id,
            model: self.model,
            usage: self.usage,
            duration,
        };
        (outcome, failure)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(text: &str, delta: bool) -> AgentEvent {
        AgentEvent::Text {
            text: text.to_string(),
            delta,
        }
    }

    fn outcome(transcript: Transcript) -> (Option<String>, Option<String>) {
        let (outcome, failure) = transcript.into_outcome("agent", Duration::ZERO);
        (outcome.result, failure)
    }

    #[test]
    fn a_final_success_clears_an_error_reported_on_the_way() {
        let mut transcript = Transcript::default();
        transcript.error("rate limited, retrying");
        transcript.succeeded(Some("DONE"));
        assert_eq!(outcome(transcript), (Some("DONE".into()), None));
    }

    #[test]
    fn an_error_fails_the_run_when_no_final_report_follows() {
        let mut transcript = Transcript::default();
        transcript.observe(&text("partial", false));
        transcript.error("boom");
        transcript.error("later");
        assert_eq!(
            outcome(transcript),
            (Some("partial".into()), Some("boom".into()))
        );
    }

    #[test]
    fn the_first_final_report_stands() {
        let mut transcript = Transcript::default();
        transcript.failed("max turns");
        assert!(transcript.ended());
        transcript.succeeded(Some("DONE"));
        assert_eq!(outcome(transcript), (None, Some("max turns".into())));
    }

    #[test]
    fn a_final_failure_keeps_the_first_error_as_its_message() {
        let mut transcript = Transcript::default();
        transcript.error("invalid API key");
        transcript.failed("exited with code 1");
        assert_eq!(outcome(transcript).1, Some("invalid API key".into()));
    }

    #[test]
    fn the_last_text_is_the_answer_when_none_is_reported() {
        let mut transcript = Transcript::default();
        transcript.observe(&text("first", false));
        transcript.observe(&AgentEvent::ToolResult {
            id: None,
            output: String::new(),
            is_error: false,
        });
        transcript.observe(&text("DO", true));
        transcript.observe(&text("NE", true));
        transcript.succeeded(Some("  "));
        assert_eq!(outcome(transcript), (Some("DONE".into()), None));
    }
}
