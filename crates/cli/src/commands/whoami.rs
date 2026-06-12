use anyhow::{anyhow, Result};
use chrono::Utc;
use clap::Args;

use crate::ascii_art::print_ascii_art;
use crate::auth::{OidcClient, TokenStorage};

#[derive(Args, Debug)]
pub struct Command {
    /// Registry URL to check
    #[arg(long)]
    registry: Option<String>,
    /// Show detailed information including token scopes
    #[arg(long)]
    detailed: bool,
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

    let oidc_client = OidcClient::new(registry_url.clone(), registry_config)?;

    let auth_file = oidc_client.get_auth_status()?;

    match auth_file {
        Some(stored_auth) if oidc_client.is_token_valid(&stored_auth.tokens) => {
            print_ascii_art();
            println!("✓ Logged in to: {registry_url}");
            println!("Username: {}", stored_auth.user.username);
            println!("Email: {}", stored_auth.user.email);
            println!("User ID: {}", stored_auth.user.id);

            if let Some(organizations) = &stored_auth.user.organizations {
                println!("\nOrganizations:");
                for org in organizations {
                    println!("  - {} ({})", org.name, org.role);
                }
            }

            if args.detailed {
                println!("\nToken Details:");
                println!("Type: {}", stored_auth.tokens.token_type);
                println!("Scopes: {}", stored_auth.tokens.scope.join(", "));

                if let Some(expires_at) = stored_auth.tokens.expires_at {
                    let now = Utc::now();
                    if expires_at > now {
                        let duration = expires_at - now;
                        let hours = duration.num_hours();
                        let minutes = duration.num_minutes() % 60;
                        println!("Expires: in {hours}h {minutes}m");
                    } else {
                        println!("Expires: ⚠️  Token expired");
                    }
                } else {
                    println!("Expires: Never");
                }

                println!(
                    "Refresh token: {}",
                    if stored_auth.tokens.refresh_token.is_some() {
                        "Available"
                    } else {
                        "None"
                    }
                );
            }
        }
        _ => {
            println!("✗ Not logged in to {registry_url}");
            println!("\nRun 'npx codemod login' to authenticate");
            std::process::exit(1);
        }
    }

    Ok(())
}
