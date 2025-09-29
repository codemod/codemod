use anyhow::{anyhow, Result};
use clap::Args;
use log::debug;
use reqwest;
use serde::{Deserialize, Serialize};
use tabled::{Table, Tabled};

use crate::auth::TokenStorage;

#[derive(Args, Debug)]
pub struct Command {
    /// Search query
    #[arg(value_name = "QUERY")]
    query: Option<String>,

    /// Filter by programming language
    #[arg(long)]
    language: Option<String>,

    /// Filter by framework
    #[arg(long)]
    framework: Option<String>,

    /// Filter by category
    #[arg(long)]
    category: Option<String>,

    /// Number of results to return
    #[arg(long, default_value = "20")]
    size: u32,

    /// Pagination offset
    #[arg(long, default_value = "0")]
    from: u32,

    /// Filter by organization scope
    #[arg(long)]
    scope: Option<String>,

    /// Registry URL
    #[arg(long)]
    registry: Option<String>,

    /// Output format
    #[arg(long, default_value = "table")]
    format: OutputFormat,
}

#[derive(clap::ValueEnum, Clone, Debug)]
enum OutputFormat {
    Table,
    Json,
    Yaml,
}

#[derive(Deserialize, Serialize, Debug)]
struct SearchResponse {
    total: u32,
    packages: Vec<Package>,
}

#[derive(Deserialize, Serialize, Debug)]
struct Package {
    id: String,
    name: String,
    scope: Option<String>,
    display_name: Option<String>,
    description: Option<String>,
    author: String,
    license: Option<String>,
    repository: Option<String>,
    homepage: Option<String>,
    keywords: Vec<String>,
    category: Option<String>,
    latest_version: Option<String>,
    download_count: u32,
    star_count: u32,
    created_at: String,
    updated_at: Option<String>,
    owner: PackageOwner,
    organization: Option<PackageOrganization>,
}

#[derive(Deserialize, Serialize, Debug)]
struct PackageOwner {
    id: String,
    username: String,
    name: String,
}

#[derive(Deserialize, Serialize, Debug)]
struct PackageOrganization {
    id: String,
    name: String,
    slug: String,
}

pub async fn handler(args: &Command) -> Result<()> {
    let storage = TokenStorage::new()?;
    let config = storage.load_config()?;

    let registry_url = args
        .registry
        .as_ref()
        .unwrap_or(&config.default_registry)
        .clone();

    debug!("Searching packages in registry: {registry_url}");

    let client = reqwest::Client::new();
    let mut url = format!("{registry_url}/api/v1/registry/search");
    let mut query_params = Vec::new();

    if let Some(query) = &args.query {
        query_params.push(("q", query.as_str()));
    }

    if let Some(language) = &args.language {
        query_params.push(("language", language.as_str()));
    }

    if let Some(framework) = &args.framework {
        query_params.push(("framework", framework.as_str()));
    }

    if let Some(category) = &args.category {
        query_params.push(("category", category.as_str()));
    }

    if let Some(scope) = &args.scope {
        query_params.push(("scope", scope.as_str()));
    }

    let size_str = args.size.to_string();
    let from_str = args.from.to_string();
    query_params.push(("size", &size_str));
    query_params.push(("from", &from_str));

    if !query_params.is_empty() {
        url.push('?');
        let query_string = query_params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        url.push_str(&query_string);
    }

    debug!("Search URL: {url}");

    let mut request = client.get(&url);

    // Add authentication header if available
    if let Ok(Some(auth)) = storage.get_auth_for_registry(&registry_url) {
        request = request.header(
            "Authorization",
            format!("Bearer {}", auth.tokens.access_token),
        );
    }

    let response = request.send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let error_text = response.text().await.unwrap_or_default();
        return Err(anyhow!(
            "Search failed with status {}: {}",
            status,
            error_text
        ));
    }

    let search_result: SearchResponse = response.json().await?;

    match args.format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&search_result)?);
        }
        OutputFormat::Yaml => {
            println!("{}", serde_yaml::to_string(&search_result)?);
        }
        OutputFormat::Table => {
            print_table(&search_result, args)?;
        }
    }

    Ok(())
}

#[derive(Tabled)]
struct PackageRow {
    #[tabled(rename = "📦 Name")]
    name: String,

    #[tabled(rename = "🔖 Version")]
    version: String,

    #[tabled(rename = "📊 Downloads")]
    downloads: String,

    #[tabled(rename = "⭐ Stars")]
    stars: String,

    #[tabled(rename = "👤 Author")]
    author: String,
}

fn print_table(result: &SearchResponse, args: &Command) -> Result<()> {
    use tabled::settings::{object::Columns, Alignment, Modify, Style};

    if result.packages.is_empty() {
        println!("No packages found.");
        return Ok(());
    }

    println!("Found {} packages:\n", result.total);

    let rows: Vec<PackageRow> = result
        .packages
        .iter()
        .map(|package| {
            let name = match &package.scope {
                Some(scope) => format!("@{}/{}", scope, package.name),
                None => package.name.clone(),
            };

            let version = package
                .latest_version
                .as_ref()
                .map(|v| v.clone())
                .unwrap_or_else(|| "N/A".to_string());

            let downloads = format_number(package.download_count);
            let stars = format_number(package.star_count);
            let author = package.author.clone();

            PackageRow {
                name,
                version,
                downloads,
                stars,
                author,
            }
        })
        .collect();

    let mut table = Table::new(rows);
    table
        .with(Style::rounded())
        .with(Modify::new(Columns::new(..)).with(Alignment::left())); // align all columns left

    println!("{table}");

    if result.total as usize > result.packages.len() {
        let shown = args.from + result.packages.len() as u32;
        println!("\nShowing {} of {} packages", shown, result.total);

        if shown < result.total {
            println!("Use --from {shown} to see more results");
        }
    }

    Ok(())
}
fn format_number(num: u32) -> String {
    if num >= 1_000_000 {
        format_suffix(num, 1_000_000.0, "M")
    } else if num >= 1_000 {
        format_suffix(num, 1_000.0, "K")
    } else {
        num.to_string()
    }
}

fn format_suffix(num: u32, divisor: f64, suffix: &str) -> String {
    let value = num as f64 / divisor;
    if value.fract() == 0.0 {
        format!("{value:.0}{suffix}")
    } else {
        format!("{value:.1}{suffix}")
    }
}
