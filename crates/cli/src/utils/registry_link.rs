use butterflow_core::registry::ResolvedPackage;
use butterflow_core::registry_link::resolve_registry_link_url;

pub fn registry_link_url_for_resolved_package(
    default_registry_url: &str,
    resolved_package: &ResolvedPackage,
) -> String {
    if let Some(metadata) = &resolved_package.registry_metadata {
        let registry_base = if metadata.registry_base_url.trim().is_empty() {
            default_registry_url
        } else {
            metadata.registry_base_url.as_str()
        };

        return resolve_registry_link_url(
            registry_base,
            Some(metadata.package_web_path.as_str()),
            metadata.access.as_deref(),
        );
    }

    resolve_registry_link_url(default_registry_url, None, None)
}

pub fn registry_link_url_for_local_run(default_registry_url: &str) -> String {
    resolve_registry_link_url(default_registry_url, None, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use butterflow_core::registry::{PackageSpec, RegistryPackageMetadata, ResolvedPackage};
    use std::path::PathBuf;

    fn resolved_package(metadata: Option<RegistryPackageMetadata>) -> ResolvedPackage {
        ResolvedPackage {
            spec: PackageSpec {
                scope: None,
                name: "debarrel".to_string(),
                version: None,
            },
            version: "1.0.0".to_string(),
            package_dir: PathBuf::from("/tmp/debarrel"),
            dry_run_only: false,
            registry_metadata: metadata,
        }
    }

    #[test]
    fn public_package_links_to_package_page() {
        let package = resolved_package(Some(RegistryPackageMetadata {
            registry_base_url: "https://app.codemod.com".to_string(),
            package_web_path: "debarrel".to_string(),
            access: Some("public".to_string()),
        }));

        assert_eq!(
            registry_link_url_for_resolved_package("https://app.codemod.com", &package),
            "https://app.codemod.com/registry/debarrel"
        );
    }

    #[test]
    fn missing_metadata_falls_back_to_home() {
        let package = resolved_package(None);

        assert_eq!(
            registry_link_url_for_resolved_package("https://app.codemod.com", &package),
            "https://app.codemod.com/registry"
        );
    }

    #[test]
    fn empty_metadata_base_falls_back_to_default_registry() {
        let package = resolved_package(Some(RegistryPackageMetadata {
            registry_base_url: "   ".to_string(),
            package_web_path: "debarrel".to_string(),
            access: Some("public".to_string()),
        }));

        assert_eq!(
            registry_link_url_for_resolved_package("https://app.codemod.com", &package),
            "https://app.codemod.com/registry/debarrel"
        );
    }

    #[test]
    fn empty_package_path_falls_back_to_home() {
        let package = resolved_package(Some(RegistryPackageMetadata {
            registry_base_url: "https://app.codemod.com".to_string(),
            package_web_path: String::new(),
            access: Some("public".to_string()),
        }));

        assert_eq!(
            registry_link_url_for_resolved_package("https://app.codemod.com", &package),
            "https://app.codemod.com/registry"
        );
    }

    #[test]
    fn local_run_always_links_to_home() {
        assert_eq!(
            registry_link_url_for_local_run("https://app.codemod.com"),
            "https://app.codemod.com/registry"
        );
        assert_eq!(
            registry_link_url_for_local_run(""),
            "https://app.codemod.com/registry"
        );
    }
}
