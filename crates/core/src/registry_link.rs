/// Build the registry home URL for a registry base such as `https://app.codemod.com`.
pub fn registry_home_url(registry_base: &str) -> String {
    format!("{}/registry", registry_base.trim_end_matches('/'))
}

/// Build a registry package page URL for a published package.
pub fn registry_package_url(registry_base: &str, package_path: &str) -> String {
    format!(
        "{}/{}",
        registry_home_url(registry_base),
        package_path.trim_start_matches('/')
    )
}

/// Whether a published registry package should deep-link to its package page.
pub fn links_to_registry_package_page(access: Option<&str>) -> bool {
    match access.map(str::to_ascii_lowercase) {
        None => true,
        Some(access) if access == "pro" || access == "public" => true,
        Some(access) if access == "private" => false,
        Some(_) => false,
    }
}

/// Resolve the URL the report UI Registry button should open.
pub fn resolve_registry_link_url(
    registry_base: &str,
    package_path: Option<&str>,
    access: Option<&str>,
) -> String {
    if let Some(package_path) = package_path.filter(|path| !path.is_empty()) {
        if links_to_registry_package_page(access) {
            return registry_package_url(registry_base, package_path);
        }
    }

    registry_home_url(registry_base)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_registry_package_links_to_package_page() {
        let url = resolve_registry_link_url(
            "https://app.codemod.com",
            Some("debarrel"),
            Some("public"),
        );

        assert_eq!(url, "https://app.codemod.com/registry/debarrel");
    }

    #[test]
    fn pro_registry_package_links_to_package_page() {
        let url = resolve_registry_link_url(
            "https://app.codemod.com",
            Some("debarrel"),
            Some("pro"),
        );

        assert_eq!(url, "https://app.codemod.com/registry/debarrel");
    }

    #[test]
    fn private_registry_package_links_to_registry_home() {
        let url = resolve_registry_link_url(
            "https://app.codemod.com",
            Some("@acme/private-codemod"),
            Some("private"),
        );

        assert_eq!(url, "https://app.codemod.com/registry");
    }

    #[test]
    fn local_runs_link_to_registry_home() {
        let url = resolve_registry_link_url("https://app.codemod.com", None, None);

        assert_eq!(url, "https://app.codemod.com/registry");
    }
}
