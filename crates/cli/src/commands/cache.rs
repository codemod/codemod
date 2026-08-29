use anyhow::{Result, anyhow};
use butterflow_core::utils::get_cache_dir;
use clap::{Args, Subcommand};
use log::info;
use std::fs;
use std::path::{Path, PathBuf};
use tabled::settings::{Alignment, Modify, Style, object::Columns};
use tabled::{Table, Tabled};
use walkdir::WalkDir;
#[derive(Args, Debug)]
pub struct Command {
    #[command(subcommand)]
    action: CacheAction,
}

#[derive(Subcommand, Debug)]
enum CacheAction {
    /// Show cache information and statistics
    Info,
    /// List cached packages
    List {
        /// Show package details
        #[arg(long)]
        detailed: bool,
    },
    /// Clear cache for specific package
    Clear {
        /// Package name (e.g., @org/package or package)
        #[arg(value_name = "PACKAGE")]
        package: Option<String>,
        /// Clear all cached packages
        #[arg(long)]
        all: bool,
    },
    /// Prune old or unused cache entries
    Prune {
        /// Maximum age in days to keep
        #[arg(long, default_value = "30")]
        max_age: u32,
        /// Dry run - show what would be removed
        #[arg(long)]
        dry_run: bool,
    },
}

pub async fn handler(args: &Command) -> Result<()> {
    match &args.action {
        CacheAction::Info => show_cache_info().await,
        CacheAction::List { detailed } => list_cached_packages(*detailed).await,
        CacheAction::Clear { package, all } => clear_cache(package.as_deref(), *all).await,
        CacheAction::Prune { max_age, dry_run } => prune_cache(*max_age, *dry_run).await,
    }
}

async fn show_cache_info() -> Result<()> {
    let cache_dir = get_cache_dir()?;

    if !cache_dir.exists() {
        println!("No cache directory found. Cache is empty.");
        return Ok(());
    }

    let cache_stats = calculate_cache_stats(&cache_dir)?;

    println!("📦 Package Cache Information");
    println!("   📁 Location: {}", cache_dir.display());
    println!("   📊 Total packages: {}", cache_stats.package_count);
    println!("   📊 Total versions: {}", cache_stats.version_count);
    println!("   💾 Total size: {}", format_size(cache_stats.total_size));

    if cache_stats.package_count > 0 {
        println!(
            "   📊 Average size per package: {}",
            format_size(cache_stats.total_size / cache_stats.package_count as u64)
        );
    }

    Ok(())
}

#[derive(Tabled)]
struct CachedPackageRow {
    #[tabled(rename = "📦 Name")]
    name: String,
    #[tabled(rename = "🔑 Versions")]
    versions: String,
    #[tabled(rename = "💾 Total Size")]
    total_size: String,
}

async fn list_cached_packages(detailed: bool) -> Result<()> {
    let cache_dir = get_cache_dir()?;

    if !cache_dir.exists() {
        println!("No cached packages found.");
        return Ok(());
    }

    let packages = discover_cached_packages(&cache_dir)?;

    if packages.is_empty() {
        println!("No cached packages found.");
        return Ok(());
    }

    println!("📦 Cached Packages ({}):", packages.len());

    let rows = if detailed {
        packages
            .iter()
            .map(|p| CachedPackageRow {
                name: format!("{}/{}", p.scope.clone().unwrap_or_default(), p.name),
                versions: p
                    .versions
                    .iter()
                    .map(|v| {
                        let modified_str = humantime::format_duration(
                            std::time::SystemTime::now()
                                .duration_since(v.modified)
                                .unwrap_or_default(),
                        );

                        format!(
                            "{} ({}, {} ago)",
                            v.version,
                            format_size(v.size),
                            modified_str
                        )
                    })
                    .collect::<Vec<String>>()
                    .join("\n "),
                total_size: format_size(p.total_size),
            })
            .collect::<Vec<CachedPackageRow>>()
    } else {
        packages
            .iter()
            .map(|p| CachedPackageRow {
                name: format!("{}/{}", p.scope.clone().unwrap_or_default(), p.name),
                versions: format!("{} versions", p.versions.len()),
                total_size: format_size(p.total_size),
            })
            .collect::<Vec<CachedPackageRow>>()
    };

    let mut table = Table::new(rows);
    table
        .with(Style::rounded())
        .with(Modify::new(Columns::new(..)).with(Alignment::left())); // align all columns left
    println!("{table}");

    Ok(())
}

async fn clear_cache(package: Option<&str>, all: bool) -> Result<()> {
    let cache_dir = get_cache_dir()?;

    if !cache_dir.exists() {
        println!("No cache directory found.");
        return Ok(());
    }

    if all {
        info!("Clearing all cached packages...");
        fs::remove_dir_all(&cache_dir)?;
        fs::create_dir_all(&cache_dir)?;
        println!("✓ All cached packages cleared");
    } else if let Some(package_name) = package {
        let package_spec = parse_package_name(package_name)?;
        let package_dir = get_package_dir(&cache_dir, &package_spec);

        if package_dir.exists() {
            info!("Clearing cache for package: {package_name}");
            fs::remove_dir_all(&package_dir)?;
            println!("✓ Cache cleared for package: {package_name}");
        } else {
            println!("Package not found in cache: {package_name}");
        }
    } else {
        return Err(anyhow!("Either specify a package name or use --all"));
    }

    Ok(())
}

async fn prune_cache(max_age_days: u32, dry_run: bool) -> Result<()> {
    let cache_dir = get_cache_dir()?;

    if !cache_dir.exists() {
        println!("No cache directory found.");
        return Ok(());
    }

    let cutoff_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs()
        - (max_age_days as u64 * 24 * 60 * 60);

    let mut pruned_count = 0;
    let mut pruned_size = 0u64;

    for entry in WalkDir::new(&cache_dir)
        .min_depth(3)
        .max_depth(3)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_dir())
    {
        if let Ok(metadata) = entry.metadata()
            && let Ok(modified) = metadata.modified()
            && let Ok(modified_time) = modified.duration_since(std::time::UNIX_EPOCH)
            && modified_time.as_secs() < cutoff_time
        {
            let size = calculate_dir_size(entry.path())?;

            if dry_run {
                println!(
                    "Would remove: {} ({})",
                    entry.path().display(),
                    format_size(size)
                );
            } else {
                info!("Removing old cache entry: {}", entry.path().display());
                fs::remove_dir_all(entry.path())?;
            }

            pruned_count += 1;
            pruned_size += size;
        }
    }

    if dry_run {
        println!(
            "Would prune {} packages ({}) older than {} days",
            pruned_count,
            format_size(pruned_size),
            max_age_days
        );
    } else {
        println!(
            "✓ Pruned {} packages ({}) older than {} days",
            pruned_count,
            format_size(pruned_size),
            max_age_days
        );
    }

    Ok(())
}

struct CacheStats {
    package_count: usize,
    version_count: usize,
    total_size: u64,
}

struct CachedPackage {
    scope: Option<String>,
    name: String,
    versions: Vec<CachedVersion>,
    total_size: u64,
}

struct CachedVersion {
    version: String,
    #[allow(dead_code)]
    path: PathBuf,
    size: u64,
    modified: std::time::SystemTime,
}

struct PackageSpec {
    scope: Option<String>,
    name: String,
}

fn calculate_cache_stats(cache_dir: &Path) -> Result<CacheStats> {
    let packages = discover_cached_packages(cache_dir)?;

    let package_count = packages.len();
    let version_count = packages.iter().map(|p| p.versions.len()).sum();
    let total_size = packages.iter().map(|p| p.total_size).sum();

    Ok(CacheStats {
        package_count,
        version_count,
        total_size,
    })
}

fn discover_cached_packages(cache_dir: &Path) -> Result<Vec<CachedPackage>> {
    let mut packages = Vec::new();

    // Handle scoped packages (@scope/name)
    for scope_entry in fs::read_dir(cache_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
    {
        let scope_name = scope_entry.file_name().to_string_lossy().to_string();

        if scope_name.starts_with('@') {
            // Scoped packages
            for package_entry in fs::read_dir(scope_entry.path())?
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            {
                let package_name = package_entry.file_name().to_string_lossy().to_string();
                let versions = discover_package_versions(&package_entry.path())?;
                let total_size = versions.iter().map(|v| v.size).sum();

                packages.push(CachedPackage {
                    scope: Some(scope_name.clone()),
                    name: package_name,
                    versions,
                    total_size,
                });
            }
        } else if scope_name == "global" {
            // Global packages
            for package_entry in fs::read_dir(scope_entry.path())?
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            {
                let package_name = package_entry.file_name().to_string_lossy().to_string();
                let versions = discover_package_versions(&package_entry.path())?;
                let total_size = versions.iter().map(|v| v.size).sum();

                packages.push(CachedPackage {
                    scope: None,
                    name: package_name,
                    versions,
                    total_size,
                });
            }
        }
    }

    packages.sort_by(|a, b| match (&a.scope, &b.scope) {
        (Some(scope_a), Some(scope_b)) => scope_a.cmp(scope_b).then(a.name.cmp(&b.name)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.name.cmp(&b.name),
    });

    Ok(packages)
}

fn discover_package_versions(package_dir: &Path) -> Result<Vec<CachedVersion>> {
    let mut versions = Vec::new();

    for version_entry in fs::read_dir(package_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
    {
        let version_name = version_entry.file_name().to_string_lossy().to_string();
        let metadata = version_entry.metadata()?;
        let size = calculate_dir_size(&version_entry.path())?;

        versions.push(CachedVersion {
            version: version_name,
            path: version_entry.path().to_path_buf(),
            size,
            modified: metadata.modified().unwrap_or(std::time::UNIX_EPOCH),
        });
    }

    versions.sort_by(|a, b| a.version.cmp(&b.version));
    Ok(versions)
}

fn calculate_dir_size(dir: &Path) -> Result<u64> {
    let mut total_size = 0;

    for entry in WalkDir::new(dir) {
        let entry = entry?;
        if entry.file_type().is_file() {
            total_size += entry.metadata()?.len();
        }
    }

    Ok(total_size)
}

fn parse_package_name(package: &str) -> Result<PackageSpec> {
    if package.starts_with('@') {
        let parts: Vec<&str> = package.splitn(2, '/').collect();
        if parts.len() == 2 {
            Ok(PackageSpec {
                scope: Some(parts[0].to_string()),
                name: parts[1].to_string(),
            })
        } else {
            Err(anyhow!("Invalid scoped package name: {}", package))
        }
    } else {
        Ok(PackageSpec {
            scope: None,
            name: package.to_string(),
        })
    }
}

fn get_package_dir(cache_dir: &Path, spec: &PackageSpec) -> PathBuf {
    if let Some(scope) = &spec.scope {
        cache_dir.join(scope).join(&spec.name)
    } else {
        cache_dir.join("global").join(&spec.name)
    }
}

fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit_index = 0;

    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    if unit_index == 0 {
        format!("{} {}", bytes, UNITS[unit_index])
    } else {
        format!("{:.1} {}", size, UNITS[unit_index])
    }
}
