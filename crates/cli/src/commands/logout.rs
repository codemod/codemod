use anyhow::{Result, anyhow};
use clap::Args;
use log::info;

use crate::auth::{OidcClient, TokenStorage};

#[derive(Args, Debug)]
pub struct Command {
    /// Registry URL to logout from
    #[arg(long)]
    registry: Option<String>,
    /// Logout from all registries
    #[arg(long)]
    all: bool,
}

pub async fn handler(args: &Command) -> Result<()> {
    let storage = TokenStorage::new()?;
    let config = storage.load_config()?;

    if args.all {
        info!("Logging out from all registries...");

        // Clear all auth files
        for registry_url in config.registries.keys() {
            let registry_config = config.registries.get(registry_url).unwrap();
            let oidc_client = OidcClient::new(registry_url.clone(), registry_config.clone())?;
            let _ = oidc_client.logout(); // Ignore errors for individual registries
        }

        // Clear entire cache
        storage.clear_cache()?;

        println!("✓ Logged out from all registries");
        println!("✓ Cleared all package caches");

        return Ok(());
    }

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

    // Check if logged in
    if oidc_client.get_auth_status()?.is_none() {
        println!("✓ Not logged in to {registry_url}");
        return Ok(());
    }

    oidc_client.logout()?;
    Ok(())
}
