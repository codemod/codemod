use anyhow::{Context, Result, anyhow};
use chrono::{Duration, Utc};
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server, StatusCode};
use log::{debug, info, warn};
use oauth2::basic::BasicClient;
use oauth2::reqwest::async_http_client;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, CsrfToken, PkceCodeChallenge, PkceCodeVerifier,
    RedirectUrl, Scope, TokenResponse, TokenUrl,
};
use rand::distributions::Alphanumeric;
use rand::{Rng, thread_rng};
use reqwest::Client;
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use tokio::sync::oneshot;

use crate::auth::storage::{RegistryAuthConfig, StoredAuth, TokenStorage};
use crate::auth::types::{AuthTokens, UserInfo};

pub struct OidcClient {
    registry_url: String,
    config: RegistryAuthConfig,
    storage: TokenStorage,
    http_client: Client,
}

#[derive(Debug)]
struct CallbackData {
    code: String,
    state: String,
}

impl OidcClient {
    pub fn new(registry_url: String, config: RegistryAuthConfig) -> Result<Self> {
        let storage = TokenStorage::new()?;
        let http_client = Client::new();

        Ok(Self {
            registry_url,
            config,
            storage,
            http_client,
        })
    }

    pub async fn login(&self) -> Result<StoredAuth> {
        info!("Starting OIDC authentication flow...");

        // Check if already authenticated
        if let Some(stored_auth) = self.storage.load_auth(&self.registry_url)? {
            if self.is_token_valid(&stored_auth.tokens) {
                info!("Already authenticated and token is valid");
                return Ok(stored_auth);
            } else if stored_auth.tokens.refresh_token.is_some() {
                info!("Token expired, attempting refresh...");
                if let Ok(refreshed_auth) = self.refresh_token(&stored_auth).await {
                    return Ok(refreshed_auth);
                }
                warn!("Token refresh failed, starting new authentication flow");
            }
        }

        // Generate PKCE parameters
        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
        let state = CsrfToken::new(Self::generate_random_string(32));

        // Find available port for callback server
        let callback_port = self.find_available_port(8090, 9000)?;
        let redirect_uri = format!("http://localhost:{callback_port}/callback");

        info!("Starting local server on http://localhost:{callback_port}");

        // Start callback server
        let (tx, rx) = oneshot::channel::<CallbackData>();
        let server_future = self.start_callback_server(callback_port, tx);

        // Build authorization URL
        let client = BasicClient::new(
            ClientId::new(self.config.client_id.clone()),
            None,
            AuthUrl::new(self.config.auth_url.clone())?,
            Some(TokenUrl::new(self.config.token_url.clone())?),
        )
        .set_redirect_uri(RedirectUrl::new(redirect_uri.clone())?);

        let mut auth_request = client
            .authorize_url(|| state.clone())
            .set_pkce_challenge(pkce_challenge);

        for scope in &self.config.scopes {
            auth_request = auth_request.add_scope(Scope::new(scope.clone()));
        }

        let (auth_url, _) = auth_request.url();

        // Open browser
        info!("Opening browser to authorize CLI access...");
        info!("If the browser doesn't open automatically, visit: {auth_url}");

        if let Err(e) = open::that(auth_url.as_str()) {
            warn!("Failed to open browser automatically: {e}");
            println!("Please open the following URL in your browser:");
            println!("{auth_url}");
        }

        // Wait for callback or server completion
        tokio::select! {
            callback_result = rx => {
                match callback_result {
                    Ok(callback_data) => {
                        info!("Authorization code received");

                        // Validate state parameter
                        if callback_data.state != *state.secret() {
                            return Err(anyhow!("Invalid state parameter - possible CSRF attack"));
                        }

                        // Exchange code for token
                        let tokens = self.exchange_code_for_token(
                            &client,
                            callback_data.code,
                            pkce_verifier,
                        ).await?;

                        // Get user info
                        let user_info = self.get_user_info(&tokens.access_token).await?;

                        let stored_auth = StoredAuth {
                            tokens,
                            user: user_info,
                            registry: self.registry_url.clone(),
                        };

                        // Save authentication
                        self.storage.save_auth(&stored_auth)?;

                        info!("✓ Successfully logged in as {}", stored_auth.user.username);
                        println!("✓ Successfully logged in as {}", stored_auth.user.username);

                        Ok(stored_auth)
                    }
                    Err(_) => Err(anyhow!("Authentication was cancelled or failed"))
                }
            }
            server_result = server_future => {
                match server_result {
                    Ok(_) => Err(anyhow!("Server completed without receiving callback")),
                    Err(e) => Err(anyhow!("Server error: {}", e))
                }
            }
        }
    }

    pub async fn refresh_token(&self, stored_auth: &StoredAuth) -> Result<StoredAuth> {
        let refresh_token = stored_auth
            .tokens
            .refresh_token
            .as_ref()
            .ok_or_else(|| anyhow!("No refresh token available"))?;

        debug!("Refreshing access token...");

        let client = BasicClient::new(
            ClientId::new(self.config.client_id.clone()),
            None,
            AuthUrl::new(self.config.auth_url.clone())?,
            Some(TokenUrl::new(self.config.token_url.clone())?),
        );

        let token_response = client
            .exchange_refresh_token(&oauth2::RefreshToken::new(refresh_token.clone()))
            .request_async(async_http_client)
            .await
            .context("Failed to refresh token")?;

        let expires_at = token_response
            .expires_in()
            .map(|duration| Utc::now() + Duration::seconds(duration.as_secs() as i64));

        let new_tokens = AuthTokens {
            access_token: token_response.access_token().secret().clone(),
            refresh_token: token_response
                .refresh_token()
                .map(|t| t.secret().clone())
                .or_else(|| stored_auth.tokens.refresh_token.clone()),
            expires_at,
            scope: stored_auth.tokens.scope.clone(),
            token_type: "Bearer".to_string(),
        };

        let updated_auth = StoredAuth {
            tokens: new_tokens,
            user: stored_auth.user.clone(),
            registry: stored_auth.registry.clone(),
        };

        self.storage.save_auth(&updated_auth)?;

        info!("✓ Token refreshed successfully");
        Ok(updated_auth)
    }

    pub fn logout(&self) -> Result<()> {
        info!("Logging out...");

        self.storage.remove_auth(&self.registry_url)?;
        self.storage.clear_cache()?;

        info!("✓ Logged out successfully");
        println!("✓ Logged out successfully");
        println!("✓ Cleared private package cache");

        Ok(())
    }

    pub fn get_auth_status(&self) -> Result<Option<StoredAuth>> {
        self.storage.load_auth(&self.registry_url)
    }

    pub fn is_token_valid(&self, tokens: &AuthTokens) -> bool {
        if let Some(expires_at) = tokens.expires_at {
            // Consider token invalid if it expires within the next 5 minutes
            let buffer = Duration::minutes(5);
            Utc::now() + buffer < expires_at
        } else {
            // If no expiration time, assume it's valid
            true
        }
    }

    async fn exchange_code_for_token(
        &self,
        client: &BasicClient,
        code: String,
        pkce_verifier: PkceCodeVerifier,
    ) -> Result<AuthTokens> {
        debug!("Exchanging authorization code for access token...");

        let token_response = client
            .exchange_code(AuthorizationCode::new(code))
            .set_pkce_verifier(pkce_verifier)
            .request_async(async_http_client)
            .await
            .context("Failed to exchange authorization code for token")?;

        let expires_at = token_response
            .expires_in()
            .map(|duration| Utc::now() + Duration::seconds(duration.as_secs() as i64));

        let scope = token_response
            .scopes()
            .map(|scopes| scopes.iter().map(|s| s.to_string()).collect())
            .unwrap_or_else(|| self.config.scopes.clone());

        Ok(AuthTokens {
            access_token: token_response.access_token().secret().clone(),
            refresh_token: token_response.refresh_token().map(|t| t.secret().clone()),
            expires_at,
            scope,
            token_type: "Bearer".to_string(),
        })
    }

    async fn get_user_info(&self, access_token: &str) -> Result<UserInfo> {
        debug!("Fetching user information...");

        let user_info_url = format!("{}/api/auth/oauth2/userinfo", self.registry_url);

        let response = self
            .http_client
            .get(&user_info_url)
            .bearer_auth(access_token)
            .send()
            .await
            .context("Failed to fetch user information")?;

        if !response.status().is_success() {
            return Err(anyhow!(
                "Failed to fetch user info: HTTP {}",
                response.status()
            ));
        }

        let user_info: UserInfo = response
            .json()
            .await
            .context("Failed to parse user information")?;

        Ok(user_info)
    }

    fn find_available_port(&self, start: u16, end: u16) -> Result<u16> {
        for port in start..=end {
            if TcpListener::bind(("127.0.0.1", port)).is_ok() {
                return Ok(port);
            }
        }
        Err(anyhow!(
            "No available ports in range {}-{}. Please close other applications and try again.",
            start,
            end
        ))
    }

    async fn start_callback_server(
        &self,
        port: u16,
        tx: oneshot::Sender<CallbackData>,
    ) -> Result<()> {
        let tx = Arc::new(tokio::sync::Mutex::new(Some(tx)));

        let make_service = make_service_fn(move |_conn| {
            let tx = tx.clone();
            async move {
                Ok::<_, Infallible>(service_fn(move |req| {
                    let tx = tx.clone();
                    async move { handle_callback(req, tx).await }
                }))
            }
        });

        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let server = Server::bind(&addr).serve(make_service);

        debug!("Callback server listening on {addr}");

        if let Err(e) = server.await {
            return Err(anyhow!("Server error: {}", e));
        }

        Ok(())
    }

    fn generate_random_string(length: usize) -> String {
        thread_rng()
            .sample_iter(&Alphanumeric)
            .take(length)
            .map(char::from)
            .collect()
    }
}

async fn handle_callback(
    req: Request<Body>,
    tx: Arc<tokio::sync::Mutex<Option<oneshot::Sender<CallbackData>>>>,
) -> Result<Response<Body>, Infallible> {
    match req.method() {
        &Method::GET => {
            let uri = req.uri();

            if uri.path() == "/callback"
                && let Some(query) = uri.query()
            {
                let params: HashMap<String, String> = url::form_urlencoded::parse(query.as_bytes())
                    .into_owned()
                    .collect();

                if let (Some(code), Some(state)) = (params.get("code"), params.get("state")) {
                    // Send callback data
                    let mut tx_guard = tx.lock().await;
                    if let Some(sender) = tx_guard.take() {
                        let _ = sender.send(CallbackData {
                            code: code.clone(),
                            state: state.clone(),
                        });
                    }

                    // Return success page
                    let html = format!(
                        include_str!("html/post-login.html.txt"),
                        title = "You're signed in",
                        description = "You can now close this browser tab and return to the CLI."
                    );

                    return Ok(Response::builder()
                        .status(StatusCode::OK)
                        .header("Content-Type", "text/html")
                        .body(Body::from(html))
                        .unwrap());
                } else if params.contains_key("error") {
                    let error = params.get("error").unwrap();
                    let default_error = "Unknown error".to_string();
                    let error_description =
                        params.get("error_description").unwrap_or(&default_error);

                    let html = format!(
                        include_str!("html/post-login.html.txt"),
                        title = "Authentication Failed",
                        description = "Error: {error}<br>Description: {error_description}"
                    );

                    return Ok(Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .header("Content-Type", "text/html")
                        .body(Body::from(html))
                        .unwrap());
                }
            }

            // Default response for unknown paths
            Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::from("Not Found"))
                .unwrap())
        }
        _ => Ok(Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .body(Body::from("Method Not Allowed"))
            .unwrap()),
    }
}
