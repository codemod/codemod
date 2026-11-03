use async_trait::async_trait;
use std::collections::HashMap;

use crate::send_event::{PostHogSender, TelemetryError};

/// Allowlisted person traits for a logged-in user.
///
/// Only these keys can ever reach PostHog as person properties; anything else
/// about the user (username, organizations, tokens) stays out by construction.
/// The registry userinfo endpoint does not return a display name today, so this
/// carries email only until that changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonTraits {
    email: String,
}

impl PersonTraits {
    /// Blank or whitespace-only emails are dropped rather than sent as `""`.
    pub fn from_email(email: &str) -> Option<Self> {
        let email = email.trim();
        if email.is_empty() {
            None
        } else {
            Some(Self {
                email: email.to_string(),
            })
        }
    }

    pub fn email(&self) -> &str {
        &self.email
    }

    /// The exact `$set` object sent to PostHog.
    pub fn set_properties(&self) -> HashMap<&'static str, String> {
        HashMap::from([("email", self.email.clone())])
    }
}

/// Delivers person traits to PostHog.
///
/// Deliberately separate from `TelemetrySender`: person properties must never
/// fan out through `CompositeSender` to Scarf, which encodes every property in
/// the request URL. Only `PostHogSender` implements this.
#[async_trait]
pub trait PersonTraitsSender: Send + Sync {
    async fn try_send_person_set(&self, traits: &PersonTraits) -> Result<(), TelemetryError>;
}

#[async_trait]
impl PersonTraitsSender for PostHogSender {
    /// Sends a `$set` event for the sender's configured `distinct_id`.
    ///
    /// This is a plain person-property update, not an identify: no
    /// `$anon_distinct_id` or alias fields, so no anonymous history is merged
    /// into the user. Unlike regular events the name is not namespaced, because
    /// `$set` is a PostHog built-in.
    async fn try_send_person_set(&self, traits: &PersonTraits) -> Result<(), TelemetryError> {
        let distinct_id = self.options.distinct_id.clone();
        let mut event = posthog_rs::Event::new("$set".to_string(), distinct_id.clone());
        event.insert_prop("distinct_id", distinct_id)?;
        event.insert_prop("$set", traits.set_properties())?;
        event.insert_prop("$process_person_profile", true)?;
        event.insert_prop("$geoip_disable", true)?;

        let summary = self.client.capture_immediate(event).await?;
        if summary.submitted() != 1 {
            return Err(TelemetryError::NoEventSubmitted);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::send_event::test_support::{posthog_test_sender, request_json};

    #[test]
    fn blank_email_is_dropped() {
        assert!(PersonTraits::from_email("  ").is_none());
        assert!(PersonTraits::from_email("").is_none());
        assert_eq!(
            PersonTraits::from_email(" ada@example.com ")
                .unwrap()
                .email(),
            "ada@example.com"
        );
    }

    #[tokio::test]
    async fn person_set_sends_only_allowlisted_traits_for_the_user_id() {
        let (sender, server) = posthog_test_sender(vec!["200 OK"], "user-123").await;

        sender
            .try_send_person_set(&PersonTraits::from_email("ada@example.com").unwrap())
            .await
            .expect("person set should be accepted");

        let requests = server.finish().await;
        let payload = request_json(&requests[0]);
        let event = &payload["batch"][0];

        assert_eq!(event["event"], "$set");
        assert_eq!(event["distinct_id"], "user-123");
        let properties = &event["properties"];
        assert_eq!(properties["distinct_id"], "user-123");
        assert_eq!(
            properties["$set"],
            serde_json::json!({ "email": "ada@example.com" })
        );
        assert_eq!(properties["$process_person_profile"], true);
        assert_eq!(properties["$geoip_disable"], true);
        assert!(properties.get("$anon_distinct_id").is_none());
        assert!(properties.get("$set_once").is_none());
        assert!(properties.get("username").is_none());
        assert!(properties.get("name").is_none());
    }

    #[tokio::test]
    async fn person_set_surfaces_rejections() {
        let (sender, server) = posthog_test_sender(vec!["400 Bad Request"], "user-123").await;

        let error = sender
            .try_send_person_set(&PersonTraits::from_email("ada@example.com").unwrap())
            .await
            .expect_err("rejection should be reported");

        assert!(matches!(error, TelemetryError::PostHog(_)));
        server.finish().await;
    }
}
