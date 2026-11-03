use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

use crate::send_event::{PostHogSender, TelemetryError};

/// Allowlisted person traits for a logged-in user.
///
/// Only these keys can ever reach PostHog as person properties; anything else
/// about the user (username, organizations, tokens) stays out by construction.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PersonTraits {
    email: Option<String>,
    name: Option<String>,
}

impl PersonTraits {
    /// Blank or whitespace-only values are dropped rather than sent as `""`.
    pub fn new(email: Option<&str>, name: Option<&str>) -> Self {
        fn normalize(value: Option<&str>) -> Option<String> {
            value
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        }

        Self {
            email: normalize(email),
            name: normalize(name),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.email.is_none() && self.name.is_none()
    }

    /// The exact `$set` object sent to PostHog. Ordered so that the
    /// serialization, and therefore [`PersonTraits::fingerprint`], is stable.
    pub fn set_properties(&self) -> BTreeMap<&'static str, String> {
        let mut properties = BTreeMap::new();
        if let Some(email) = &self.email {
            properties.insert("email", email.clone());
        }
        if let Some(name) = &self.name {
            properties.insert("name", name.clone());
        }
        properties
    }

    /// Stable digest of the exact `$set` object; callers persist it to skip
    /// resending unchanged traits.
    pub fn fingerprint(&self) -> String {
        let serialized = serde_json::to_vec(&self.set_properties())
            .expect("string map serialization cannot fail");
        let digest = Sha256::digest(serialized);
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
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
        if traits.is_empty() {
            return Err(TelemetryError::NoPersonTraits);
        }

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
    fn blank_traits_are_dropped() {
        let traits = PersonTraits::new(Some("  "), Some(" Ada "));

        assert_eq!(
            traits.set_properties(),
            BTreeMap::from([("name", "Ada".to_string())])
        );
        assert!(PersonTraits::new(Some(""), None).is_empty());
        assert!(PersonTraits::new(None, None).is_empty());
    }

    #[test]
    fn fingerprint_tracks_the_set_object() {
        let email_only = PersonTraits::new(Some("ada@example.com"), None);
        let same_email = PersonTraits::new(Some(" ada@example.com "), None);
        let with_name = PersonTraits::new(Some("ada@example.com"), Some("Ada"));

        assert_eq!(email_only.fingerprint(), same_email.fingerprint());
        assert_ne!(email_only.fingerprint(), with_name.fingerprint());
        assert_eq!(email_only.fingerprint().len(), 64);
    }

    #[tokio::test]
    async fn person_set_sends_only_allowlisted_traits_for_the_user_id() {
        let (sender, server) = posthog_test_sender(vec!["200 OK"], "user-123").await;

        sender
            .try_send_person_set(&PersonTraits::new(Some("ada@example.com"), Some("Ada")))
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
            serde_json::json!({ "email": "ada@example.com", "name": "Ada" })
        );
        assert_eq!(properties["$process_person_profile"], true);
        assert_eq!(properties["$geoip_disable"], true);
        assert!(properties.get("$anon_distinct_id").is_none());
        assert!(properties.get("$set_once").is_none());
        assert!(properties.get("username").is_none());
    }

    #[tokio::test]
    async fn person_set_omits_missing_name() {
        let (sender, server) = posthog_test_sender(vec!["200 OK"], "user-123").await;

        sender
            .try_send_person_set(&PersonTraits::new(Some("ada@example.com"), None))
            .await
            .expect("person set should be accepted");

        let requests = server.finish().await;
        let payload = request_json(&requests[0]);

        assert_eq!(
            payload["batch"][0]["properties"]["$set"],
            serde_json::json!({ "email": "ada@example.com" })
        );
    }

    #[tokio::test]
    async fn empty_traits_are_not_sent() {
        let (sender, server) = posthog_test_sender(vec![], "user-123").await;

        let error = sender
            .try_send_person_set(&PersonTraits::default())
            .await
            .expect_err("empty traits must not produce a request");

        assert!(matches!(error, TelemetryError::NoPersonTraits));
        assert!(server.finish().await.is_empty());
    }

    #[tokio::test]
    async fn person_set_surfaces_rejections() {
        let (sender, server) = posthog_test_sender(vec!["400 Bad Request"], "user-123").await;

        let error = sender
            .try_send_person_set(&PersonTraits::new(Some("ada@example.com"), None))
            .await
            .expect_err("rejection should be reported");

        assert!(matches!(error, TelemetryError::PostHog(_)));
        server.finish().await;
    }
}
