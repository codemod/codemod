//! Associates a logged-in user's display traits with their PostHog person.
//!
//! Regular telemetry events are profile-free; this sends a separate `$set`
//! so the person keyed by `User.id` carries an email/name. It goes to PostHog
//! only, never through the composite/Scarf path, and is skipped entirely when
//! analytics are disabled (the caller never builds a PostHog sender then) or
//! when the CLI is running anonymously.

use crate::auth::types::UserInfo;
use crate::auth::TokenStorage;
use codemod_telemetry::send_person::{PersonTraits, PersonTraitsSender};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PersonTraitsSync {
    Sent,
    /// The persisted fingerprint matches; nothing to send.
    Unchanged,
    /// The user has no allowlisted traits (e.g. empty email).
    NoTraits,
    /// The fingerprint store could not be read; sending without dedupe would
    /// repeat on every invocation, so skip instead.
    StoreUnavailable,
    Failed,
}

pub(crate) async fn sync_person_traits(
    storage: &TokenStorage,
    sender: &dyn PersonTraitsSender,
    user: &UserInfo,
) -> PersonTraitsSync {
    let traits = PersonTraits::new(Some(&user.email), user.name.as_deref());
    if traits.is_empty() {
        return PersonTraitsSync::NoTraits;
    }

    let fingerprint = traits.fingerprint();
    match storage.load_person_traits_fingerprint(&user.id) {
        Ok(Some(stored)) if stored == fingerprint => return PersonTraitsSync::Unchanged,
        Ok(_) => {}
        Err(error) => {
            log::debug!("Skipping person traits sync: {error}");
            return PersonTraitsSync::StoreUnavailable;
        }
    }

    if let Err(error) = sender.try_send_person_set(&traits).await {
        log::debug!("Failed to send person traits: {error}");
        return PersonTraitsSync::Failed;
    }

    // Persist only after delivery so a failed send is retried next time.
    if let Err(error) = storage.save_person_traits_fingerprint(&user.id, &fingerprint) {
        log::debug!("Failed to persist person traits fingerprint: {error}");
    }
    PersonTraitsSync::Sent
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use codemod_telemetry::send_event::TelemetryError;
    use std::sync::Mutex;

    struct RecordingSender {
        fail: bool,
        sent: Mutex<Vec<PersonTraits>>,
    }

    impl RecordingSender {
        fn new(fail: bool) -> Self {
            Self {
                fail,
                sent: Mutex::new(Vec::new()),
            }
        }

        fn sent(&self) -> Vec<PersonTraits> {
            self.sent.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl PersonTraitsSender for RecordingSender {
        async fn try_send_person_set(&self, traits: &PersonTraits) -> Result<(), TelemetryError> {
            self.sent.lock().unwrap().push(traits.clone());
            if self.fail {
                Err(TelemetryError::NoEventSubmitted)
            } else {
                Ok(())
            }
        }
    }

    fn user(email: &str, name: Option<&str>) -> UserInfo {
        UserInfo {
            id: "user-123".to_string(),
            username: "ada".to_string(),
            email: email.to_string(),
            name: name.map(str::to_string),
            organizations: None,
        }
    }

    fn storage(dir: &tempfile::TempDir) -> TokenStorage {
        TokenStorage::with_config_dir(dir.path().to_path_buf()).expect("storage")
    }

    #[tokio::test]
    async fn sends_once_and_skips_while_traits_are_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let sender = RecordingSender::new(false);
        let user = user("ada@example.com", None);

        let first = sync_person_traits(&storage(&dir), &sender, &user).await;
        // A fresh TokenStorage models a new CLI process reading the persisted hash.
        let second = sync_person_traits(&storage(&dir), &sender, &user).await;

        assert_eq!(first, PersonTraitsSync::Sent);
        assert_eq!(second, PersonTraitsSync::Unchanged);
        assert_eq!(
            sender.sent(),
            vec![PersonTraits::new(Some("ada@example.com"), None)]
        );
    }

    #[tokio::test]
    async fn resends_when_traits_change() {
        let dir = tempfile::tempdir().unwrap();
        let sender = RecordingSender::new(false);

        sync_person_traits(&storage(&dir), &sender, &user("ada@example.com", None)).await;
        let changed = sync_person_traits(
            &storage(&dir),
            &sender,
            &user("ada@example.com", Some("Ada")),
        )
        .await;

        assert_eq!(changed, PersonTraitsSync::Sent);
        assert_eq!(sender.sent().len(), 2);
    }

    #[tokio::test]
    async fn failed_send_is_not_persisted_and_retries_next_time() {
        let dir = tempfile::tempdir().unwrap();
        let user = user("ada@example.com", None);

        let failing = RecordingSender::new(true);
        let first = sync_person_traits(&storage(&dir), &failing, &user).await;
        let working = RecordingSender::new(false);
        let second = sync_person_traits(&storage(&dir), &working, &user).await;

        assert_eq!(first, PersonTraitsSync::Failed);
        assert_eq!(second, PersonTraitsSync::Sent);
        assert_eq!(working.sent().len(), 1);
    }

    #[tokio::test]
    async fn user_without_traits_is_not_sent() {
        let dir = tempfile::tempdir().unwrap();
        let sender = RecordingSender::new(false);

        let outcome = sync_person_traits(&storage(&dir), &sender, &user("  ", None)).await;

        assert_eq!(outcome, PersonTraitsSync::NoTraits);
        assert!(sender.sent().is_empty());
    }
}
