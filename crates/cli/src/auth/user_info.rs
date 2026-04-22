use crate::auth::types::UserInfo;

pub(crate) fn format_author_from_user_info(user: &UserInfo) -> Option<String> {
    let username = user.username.trim();
    let email = user.email.trim();

    match (username.is_empty(), email.is_empty()) {
        (false, false) => Some(format!("{username} <{email}>")),
        (false, true) => Some(username.to_string()),
        (true, false) => Some(email.to_string()),
        (true, true) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(username: &str, email: &str) -> UserInfo {
        UserInfo {
            id: "user-1".to_string(),
            username: username.to_string(),
            email: email.to_string(),
            organizations: None,
        }
    }

    #[test]
    fn format_author_prefers_username_and_email() {
        assert_eq!(
            format_author_from_user_info(&user("alice", "alice@example.com")),
            Some("alice <alice@example.com>".to_string())
        );
    }

    #[test]
    fn format_author_falls_back_to_username() {
        assert_eq!(
            format_author_from_user_info(&user("alice", "")),
            Some("alice".to_string())
        );
    }
}
