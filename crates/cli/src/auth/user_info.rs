use anyhow::{anyhow, Result};
use reqwest::Client;

use crate::auth::types::UserInfo;

pub(crate) async fn fetch_user_info_with_bearer_token(
    registry_url: &str,
    access_token: &str,
) -> Result<UserInfo> {
    let client = Client::new();
    let user_info_url = format!("{registry_url}/api/auth/oauth2/userinfo");

    let response = client
        .get(&user_info_url)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| anyhow!("Failed to fetch user information: {}", e))?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "Failed to fetch user info: HTTP {}",
            response.status()
        ));
    }

    let user_info: UserInfo = response
        .json()
        .await
        .map_err(|e| anyhow!("Failed to parse user information: {}", e))?;

    Ok(user_info)
}

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
    use hyper::service::{make_service_fn, service_fn};
    use hyper::{Body, Request, Response, Server, StatusCode};
    use std::convert::Infallible;
    use std::net::TcpListener;

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

    #[test]
    fn fetch_user_info_with_bearer_token_reads_userinfo_endpoint() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let fetched_user = runtime.block_on(async {
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let address = listener.local_addr().unwrap();
            let server = Server::from_tcp(listener)
                .unwrap()
                .serve(make_service_fn(|_| async {
                    Ok::<_, Infallible>(service_fn(|request: Request<Body>| async move {
                        let response = if request.uri().path() == "/api/auth/oauth2/userinfo"
                            && request
                                .headers()
                                .get("authorization")
                                .and_then(|value| value.to_str().ok())
                                == Some("Bearer test-token")
                        {
                            Response::new(Body::from(
                                r#"{"id":"user-1","username":"alice","email":"alice@example.com","organizations":[]}"#,
                            ))
                        } else {
                            Response::builder()
                                .status(StatusCode::UNAUTHORIZED)
                                .body(Body::from("unauthorized"))
                                .unwrap()
                        };

                        Ok::<_, Infallible>(response)
                    }))
                }));

            let handle = tokio::spawn(server);
            let fetched_user = fetch_user_info_with_bearer_token(
                &format!("http://{}", address),
                "test-token",
            )
            .await
            .unwrap();
            handle.abort();
            fetched_user
        });

        assert_eq!(fetched_user.username, "alice");
        assert_eq!(fetched_user.email, "alice@example.com");
    }
}
