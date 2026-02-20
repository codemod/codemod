use anyhow::Result;
use clap::Args;
use serde::Serialize;
use tabled::{Table, Tabled};

use crate::suitability::{
    search_registry, summarize_search_coverage, MetadataCoverage, RegistrySearchPackage,
    RegistrySearchRequest, RegistrySearchResponse, SearchCoverageSummary,
};

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

#[derive(Serialize)]
struct SearchResponseOutput<'a> {
    total: u32,
    packages: Vec<PackageOutput<'a>>,
    metadata_coverage: SearchCoverageSummary,
}

#[derive(Serialize)]
struct PackageOutput<'a> {
    #[serde(flatten)]
    package: &'a RegistrySearchPackage,
    metadata_coverage: MetadataCoverage,
}

pub async fn handler(args: &Command) -> Result<()> {
    let request = RegistrySearchRequest {
        query: args.query.clone(),
        language: args.language.clone(),
        framework: args.framework.clone(),
        category: args.category.clone(),
        size: args.size,
        from: args.from,
        scope: args.scope.clone(),
        registry: args.registry.clone(),
    };

    let search_result = search_registry(request).await?;

    match args.format {
        OutputFormat::Json => {
            let output = build_search_output(&search_result.response);
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        OutputFormat::Yaml => {
            let output = build_search_output(&search_result.response);
            println!("{}", serde_yaml::to_string(&output)?);
        }
        OutputFormat::Table => {
            print_table(&search_result.response, args)?;
        }
    }

    Ok(())
}

#[derive(Tabled)]
struct PackageRow {
    #[tabled(rename = "📦 Name")]
    name: String,

    #[tabled(rename = "📊 Downloads")]
    downloads: String,

    #[tabled(rename = "⭐ Stars")]
    stars: String,

    #[tabled(rename = "👤 Author")]
    author: String,
}

fn print_table(result: &RegistrySearchResponse, args: &Command) -> Result<()> {
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

            let downloads = format_number(package.download_count);
            let stars = format_number(package.star_count);
            let author = package.author.clone();

            PackageRow {
                name,
                downloads,
                stars,
                author,
            }
        })
        .collect();

    let mut table = Table::new(rows);
    table
        .with(Style::rounded())
        .with(Modify::new(Columns::new(..)).with(Alignment::left()));

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

fn build_search_output(result: &RegistrySearchResponse) -> SearchResponseOutput<'_> {
    SearchResponseOutput {
        total: result.total,
        packages: result
            .packages
            .iter()
            .map(|package| PackageOutput {
                package,
                metadata_coverage: package.metadata_coverage(),
            })
            .collect(),
        metadata_coverage: summarize_search_coverage(&result.packages),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::suitability::{RegistryPackageOrganization, RegistryPackageOwner};

    #[test]
    fn build_search_output_embeds_metadata_coverage_summary() {
        let package = RegistrySearchPackage {
            id: "pkg-1".to_string(),
            name: "jest-to-vitest".to_string(),
            scope: Some("codemod".to_string()),
            display_name: Some("Jest to Vitest".to_string()),
            description: Some("Migrate tests".to_string()),
            author: "codemod".to_string(),
            license: Some("MIT".to_string()),
            repository: None,
            homepage: None,
            keywords: vec![],
            category: Some("testing".to_string()),
            latest_version: Some("1.0.0".to_string()),
            download_count: 1,
            star_count: 1,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: None,
            owner: RegistryPackageOwner {
                id: "owner".to_string(),
                username: "codemod".to_string(),
                name: "Codemod".to_string(),
            },
            organization: Some(RegistryPackageOrganization {
                id: "org".to_string(),
                name: "Codemod".to_string(),
                slug: "codemod".to_string(),
            }),
            frameworks: None,
            languages: None,
            version_ranges: None,
            confidence_hints: None,
            known_limits: None,
            quality_score: None,
            maintenance_score: None,
            adoption_score: None,
            pro_required: None,
            login_required: None,
        };

        let response = RegistrySearchResponse {
            total: 1,
            packages: vec![package],
        };
        let output = build_search_output(&response);

        assert_eq!(output.metadata_coverage.total_packages, 1);
        assert_eq!(output.metadata_coverage.packages_missing_contract_fields, 1);
    }
}
