use anyhow::{anyhow, Result};
use clap::Args;
use log::{info, warn};
use reqwest::Client;

use crate::ascii_art::print_ascii_art;
use crate::auth::storage::StoredAuth;
use crate::auth::types::{AuthTokens, UserInfo};
use crate::auth::{OidcClient, TokenStorage};

#[derive(Args, Debug)]
pub struct Command {
    /// Registry URL
    #[arg(long)]
    registry: Option<String>,
    /// Organization or user scope for publishing
    #[arg(long)]
    scope: Option<String>,
    /// API key for authentication (alternative to OAuth flow)
    #[arg(long)]
    api_key: Option<String>,
}

async fn validate_api_key_and_get_user_info(registry_url: &str, api_key: &str) -> Result<UserInfo> {
    let client = Client::new();
    let user_info_url = format!("{registry_url}/api/auth/oauth2/userinfo");

    let response = client
        .get(&user_info_url)
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|e| anyhow!("Failed to validate API key: {}", e))?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "API key validation failed: HTTP {} - Please check your API key",
            response.status()
        ));
    }

    let user_info: UserInfo = response
        .json()
        .await
        .map_err(|e| anyhow!("Failed to parse user information: {}", e))?;

    Ok(user_info)
}

pub async fn handler(args: &Command) -> Result<()> {
    let storage = TokenStorage::new()?;
    let config = storage.load_config()?;

    let registry_url = args
        .registry
        .as_ref()
        .unwrap_or(&config.default_registry)
        .clone();

    let registry_config = config
        .registries
        .get(&registry_url)
        .ok_or_else(|| anyhow!("Unknown registry: {}", registry_url))?
        .clone();

    info!("Authenticating with registry: {registry_url}");

    // Handle API key authentication
    if let Some(api_key) = &args.api_key {
        info!("Using API key authentication");

        // Validate API key and get user info
        let user_info = validate_api_key_and_get_user_info(&registry_url, api_key).await?;

        // Create auth tokens for API key
        let auth_tokens = AuthTokens {
            access_token: api_key.clone(),
            refresh_token: None, // API keys don't have refresh tokens
            expires_at: None,    // API keys typically don't expire
            scope: vec![
                "read".to_string(),
                "write".to_string(),
                "publish".to_string(),
            ], // Default scopes
            token_type: "Bearer".to_string(),
        };

        // Create stored auth
        let stored_auth = StoredAuth {
            tokens: auth_tokens,
            user: user_info.clone(),
            registry: registry_url.clone(),
        };

        // Save authentication
        storage.save_auth(&stored_auth)?;

        println!(
            "✓ Successfully logged in with API key as {}",
            user_info.username
        );

        if let Some(scope) = &args.scope {
            info!("Setting default publish scope to {scope}");
            // TODO: Save scope preference to config
        }

        if let Some(organizations) = &user_info.organizations {
            println!("This API key can only publish to the following organizations:");
            for org in organizations {
                println!("  - {} ({})", org.name, org.role);
            }
        }

        println!("\nYou can now publish packages using 'codemod publish'");
        print_ascii_art();
        return Ok(());
    }

    // Continue with OIDC flow if no API key provided
    let oidc_client = OidcClient::new(registry_url.clone(), registry_config)?;

    // Check current auth status
    if let Ok(Some(stored_auth)) = oidc_client.get_auth_status() {
        if oidc_client.is_token_valid(&stored_auth.tokens) {
            println!(
                "✓ Already logged in as {} ({})",
                stored_auth.user.username, registry_url
            );

            if let Some(organizations) = &stored_auth.user.organizations {
                println!(
                    "Organizations: {}",
                    organizations
                        .iter()
                        .map(|org| format!("{} ({})", org.name, org.role))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }

            println!("Use 'npx codemod@latest logout' to log out or 'npx codemod@latest login --registry <url>' to log in to a different registry.");
            return Ok(());
        } else {
            println!("Token is invalid, logging out");
            oidc_client.logout()?;
        }
    }

    // Start OIDC login flow
    match oidc_client.login().await {
        Ok(auth) => {
            if let Some(scope) = &args.scope {
                info!("Setting default publish scope to {scope}");
                // TODO: Save scope preference to config
            }

            if let Some(organizations) = &auth.user.organizations {
                println!("Available organizations:");
                for org in organizations {
                    println!("  - {} ({})", org.name, org.role);
                }
            }
            println!("\nYou can now publish packages using 'codemod publish'");
            print_ascii_art();
            Ok(())
        }
        Err(e) => {
            warn!("Authentication failed: {e}");
            Err(anyhow!("Login failed: {}", e))
        }
    }
}
