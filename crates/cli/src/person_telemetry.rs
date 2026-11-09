//! Associates a logged-in user's email with their PostHog person.
//!
//! Regular telemetry events are profile-free; this sends a separate `$set`
//! so the person keyed by `User.id` carries an email. It goes to PostHog
//! only, never through the composite/Scarf path, and is skipped entirely when
//! analytics are disabled (the caller never builds a PostHog sender then) or
//! when the CLI is running anonymously.

use crate::auth::types::UserInfo;
use crate::auth::TokenStorage;
use codemod_telemetry::send_person::{PersonTraits, PersonTraitsSender};

pub(crate) async fn sync_person_traits(
    storage: &TokenStorage,
    sender: &dyn PersonTraitsSender,
    user: &UserInfo,
) {
    let Some(traits) = PersonTraits::from_email(&user.email) else {
        return;
    };

    match storage.load_last_sent_person_email(&user.id) {
        Ok(Some(stored)) if stored == traits.email() => return,
        Ok(_) => {}
        Err(error) => {
            // Cache is best-effort; still try to send rather than skip forever.
            log::debug!("Person traits cache unavailable, sending anyway: {error}");
        }
    }

    if let Err(error) = sender.try_send_person_set(&traits).await {
        log::debug!("Failed to send person traits: {error}");
        return;
    }

    // Persist only after delivery so a failed send is retried next time.
    if let Err(error) = storage.save_last_sent_person_email(&user.id, traits.email()) {
        log::debug!("Failed to persist person traits cache: {error}");
    }
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

    fn user(email: &str) -> UserInfo {
        UserInfo {
            id: "user-123".to_string(),
            username: "ada".to_string(),
            email: email.to_string(),
            organizations: None,
        }
    }

    fn storage(dir: &tempfile::TempDir) -> TokenStorage {
        TokenStorage::with_config_dir(dir.path().to_path_buf()).expect("storage")
    }

    #[tokio::test]
    async fn sends_once_and_skips_while_email_is_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let sender = RecordingSender::new(false);
        let user = user("ada@example.com");

        sync_person_traits(&storage(&dir), &sender, &user).await;
        // A fresh TokenStorage models a new CLI process reading the persisted cache.
        sync_person_traits(&storage(&dir), &sender, &user).await;

        assert_eq!(
            sender.sent(),
            vec![PersonTraits::from_email("ada@example.com").unwrap()]
        );
    }

    #[tokio::test]
    async fn resends_when_email_changes() {
        let dir = tempfile::tempdir().unwrap();
        let sender = RecordingSender::new(false);

        sync_person_traits(&storage(&dir), &sender, &user("ada@example.com")).await;
        sync_person_traits(&storage(&dir), &sender, &user("ada@codemod.com")).await;

        assert_eq!(sender.sent().len(), 2);
        assert_eq!(sender.sent()[1].email(), "ada@codemod.com");
    }

    #[tokio::test]
    async fn failed_send_is_not_persisted_and_retries_next_time() {
        let dir = tempfile::tempdir().unwrap();
        let user = user("ada@example.com");

        let failing = RecordingSender::new(true);
        sync_person_traits(&storage(&dir), &failing, &user).await;
        let working = RecordingSender::new(false);
        sync_person_traits(&storage(&dir), &working, &user).await;

        assert_eq!(working.sent().len(), 1);
    }

    #[tokio::test]
    async fn blank_email_is_not_sent() {
        let dir = tempfile::tempdir().unwrap();
        let sender = RecordingSender::new(false);

        sync_person_traits(&storage(&dir), &sender, &user("  ")).await;

        assert!(sender.sent().is_empty());
    }
}
