use anyhow::{anyhow, Result};
use clap::Args;
use inquire::Confirm;
use log::{info, warn};
use reqwest;
use serde::{Deserialize, Serialize};

use crate::auth::TokenStorage;
use crate::webhooks;

#[derive(Args, Debug)]
pub struct Command {
    /// Package name (e.g., @org/package or package)
    #[arg(value_name = "PACKAGE")]
    package: String,

    /// Specific version to unpublish (if not provided, requires --force to unpublish all)
    #[arg(long)]
    version: Option<String>,

    /// Force unpublish all versions of the package
    #[arg(long)]
    force: bool,

    /// Target registry URL
    #[arg(long)]
    registry: Option<String>,

    /// Show what would be unpublished without actually doing it
    #[arg(long)]
    dry_run: bool,
}

#[derive(Deserialize, Serialize, Debug)]
struct UnpublishResponse {
    success: bool,
    message: String,
    unpublished: UnpublishedPackage,
}

#[derive(Deserialize, Serialize, Debug)]
struct UnpublishedPackage {
    name: String,
    scope: Option<String>,
    versions: Vec<String>,
}

pub async fn handler(args: &Command) -> Result<()> {
    // Validate arguments
    if args.version.is_none() && !args.force {
        return Err(anyhow!(
            "To unpublish all versions of a package, you must use --force flag.\n\
             Use --version to unpublish a specific version, or --force to unpublish all versions."
        ));
    }

    if args.version.is_some() && args.force {
        warn!("Both --version and --force specified. Will unpublish only the specified version.");
    }

    // Get registry configuration
    let storage = TokenStorage::new()?;
    let config = storage.load_config()?;
    let registry_url = args
        .registry
        .as_ref()
        .unwrap_or(&config.default_registry)
        .clone();

    // Check authentication
    let auth = storage
        .get_auth_for_registry(&registry_url)?
        .ok_or_else(|| {
            anyhow!(
                "Not authenticated with registry: {}. Run 'npx codemod@latest login' first.",
                registry_url
            )
        })?;

    info!(
        "Unpublishing package: {} from registry: {}",
        args.package, registry_url
    );

    if args.dry_run {
        println!("🔍 Dry run mode - showing what would be unpublished:");
        if let Some(version) = &args.version {
            println!("📦 Package: {}", args.package);
            println!("🏷️  Version: {version}");
        } else {
            println!("📦 Package: {} (all versions)", args.package);
        }
        println!("🌐 Registry: {registry_url}");
        println!();
        println!("Run without --dry-run to perform the actual unpublish operation.");
        return Ok(());
    }

    // Confirm unpublish operation
    if !confirm_unpublish(&args.package, args.version.as_deref(), args.force)? {
        println!("Unpublish operation cancelled.");
        return Ok(());
    }

    // Perform unpublish
    let response = unpublish_package(
        &registry_url,
        &args.package,
        args.version.as_deref(),
        args.force,
        &auth.tokens.access_token,
    )
    .await?;

    if !response.success {
        return Err(anyhow!("Failed to unpublish package: {}", response.message));
    }

    println!("✅ Package unpublished successfully!");
    println!("📦 {}", format_package_name(&response.unpublished));

    if response.unpublished.versions.len() == 1 {
        println!("🏷️  Version: {}", response.unpublished.versions[0]);
    } else {
        println!(
            "🏷️  Versions: {} versions removed",
            response.unpublished.versions.len()
        );
        for version in &response.unpublished.versions {
            println!("     - {version}");
        }
    }

    println!("💬 {}", response.message);

    // Send webhook event to Make.com
    if let Err(e) = webhooks::send_unpublish_webhook(
        &response.unpublished.name,
        &response.unpublished.versions,
        &auth.user.username,
        None, // GitHub URL not available in unpublish response
    )
    .await
    {
        // Log warning but don't fail the unpublish if webhook fails
        warn!("Failed to send webhook event: {}", e);
    }

    // Warning about irreversible action
    println!();
    println!("⚠️  Note: Unpublished packages cannot be republished with the same version number.");
    Ok(())
}

fn confirm_unpublish(package: &str, version: Option<&str>, force: bool) -> Result<bool> {
    let action_description = if let Some(version) = version {
        format!("unpublish version {version} of package {package}")
    } else if force {
        format!("unpublish ALL versions of package {package}")
    } else {
        return Err(anyhow!("Invalid unpublish configuration"));
    };

    println!("⚠️  You are about to {action_description}.");
    println!(
        "   This action is IRREVERSIBLE and will permanently remove the package from the registry."
    );
    println!("   Users who depend on this package may experience build failures.");
    println!();

    let confirmed = Confirm::new("Are you sure you want to continue?")
        .with_default(false)
        .prompt()
        .unwrap_or(false);

    Ok(confirmed)
}

async fn unpublish_package(
    registry_url: &str,
    package: &str,
    version: Option<&str>,
    force: bool,
    access_token: &str,
) -> Result<UnpublishResponse> {
    let client = reqwest::Client::new();
    let url = format!("{registry_url}/api/v1/registry/packages/{package}");

    let mut request = client
        .delete(&url)
        .header("Authorization", format!("Bearer {access_token}"))
        .header("User-Agent", "codemod-cli/1.0");

    // Add query parameters
    let mut query_params = Vec::new();

    if let Some(version) = version {
        query_params.push(("version", version));
    }

    if force {
        query_params.push(("force", "true"));
    }

    if !query_params.is_empty() {
        request = request.query(&query_params);
    }

    let response = request.send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();

        return match status {
            reqwest::StatusCode::NOT_FOUND => {
                if version.is_some() {
                    Err(anyhow!(
                        "Package version not found. The specified version may not exist or may have already been unpublished."
                    ))
                } else {
                    Err(anyhow!(
                        "Package not found. The package may not exist or may have already been unpublished."
                    ))
                }
            }
            reqwest::StatusCode::FORBIDDEN => {
                Err(anyhow!(
                    "Access denied. You may not have permission to unpublish this package.\n\
                     Only package owners and organization members with appropriate permissions can unpublish packages."
                ))
            }
            reqwest::StatusCode::BAD_REQUEST => {
                Err(anyhow!(
                    "Bad request: {}\n\
                     This may happen if you're trying to unpublish all versions without the --force flag.",
                    error_text
                ))
            }
            reqwest::StatusCode::UNAUTHORIZED => {
                Err(anyhow!(
                    "Authentication failed. Please run 'npx codemod@latest login' again.\n\
                     Your session may have expired."
                ))
            }
            _ => {
                Err(anyhow!(
                    "Unpublish failed with status {}: {}",
                    status,
                    error_text
                ))
            }
        };
    }

    let unpublish_response: UnpublishResponse = response.json().await?;
    Ok(unpublish_response)
}

fn format_package_name(package: &UnpublishedPackage) -> String {
    if let Some(scope) = &package.scope {
        format!("{}/{}", scope, package.name)
    } else {
        package.name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_package_name() {
        let scoped_package = UnpublishedPackage {
            name: "my-package".to_string(),
            scope: Some("@myorg".to_string()),
            versions: vec!["1.0.0".to_string()],
        };
        assert_eq!(format_package_name(&scoped_package), "@myorg/my-package");

        let unscoped_package = UnpublishedPackage {
            name: "my-package".to_string(),
            scope: None,
            versions: vec!["1.0.0".to_string()],
        };
        assert_eq!(format_package_name(&unscoped_package), "my-package");
    }
}
