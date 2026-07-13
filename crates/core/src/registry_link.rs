/// Fallback registry base used when the configured base is missing or blank.
pub const FALLBACK_REGISTRY_BASE: &str = "https://app.codemod.com";

/// Normalize a registry base URL, falling back when empty/blank.
pub fn normalize_registry_base(registry_base: &str) -> &str {
    let trimmed = registry_base.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        FALLBACK_REGISTRY_BASE
    } else {
        trimmed
    }
}

/// Build the registry home URL for a registry base such as `https://app.codemod.com`.
pub fn registry_home_url(registry_base: &str) -> String {
    format!("{}/registry", normalize_registry_base(registry_base))
}

/// Build a registry package page URL for a published package.
pub fn registry_package_url(registry_base: &str, package_path: &str) -> String {
    format!(
        "{}/{}",
        registry_home_url(registry_base),
        package_path.trim().trim_start_matches('/')
    )
}

/// Whether a published registry package should deep-link to its package page.
pub fn links_to_registry_package_page(access: Option<&str>) -> bool {
    match access.map(|value| value.trim().to_ascii_lowercase()) {
        None => true,
        Some(access) if access.is_empty() => true,
        Some(access) if access == "pro" || access == "public" => true,
        Some(access) if access == "private" => false,
        Some(_) => false,
    }
}

/// Resolve the URL the report UI Registry button should open.
///
/// Prefers a package page for public/pro packages when a non-empty package path is available.
/// Otherwise always returns the registry homepage. Never panics and never returns an empty string.
pub fn resolve_registry_link_url(
    registry_base: &str,
    package_path: Option<&str>,
    access: Option<&str>,
) -> String {
    if let Some(package_path) = package_path.map(str::trim).filter(|path| !path.is_empty()) {
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
        let url =
            resolve_registry_link_url("https://app.codemod.com", Some("debarrel"), Some("public"));

        assert_eq!(url, "https://app.codemod.com/registry/debarrel");
    }

    #[test]
    fn pro_registry_package_links_to_package_page() {
        let url =
            resolve_registry_link_url("https://app.codemod.com", Some("debarrel"), Some("pro"));

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

    #[test]
    fn empty_or_blank_package_path_falls_back_to_home() {
        assert_eq!(
            resolve_registry_link_url("https://app.codemod.com", Some(""), Some("public")),
            "https://app.codemod.com/registry"
        );
        assert_eq!(
            resolve_registry_link_url("https://app.codemod.com", Some("   "), Some("public")),
            "https://app.codemod.com/registry"
        );
    }

    #[test]
    fn empty_or_blank_registry_base_uses_fallback_home() {
        assert_eq!(
            resolve_registry_link_url("", None, None),
            "https://app.codemod.com/registry"
        );
        assert_eq!(
            resolve_registry_link_url("   ", Some("debarrel"), Some("public")),
            "https://app.codemod.com/registry/debarrel"
        );
    }

    #[test]
    fn unknown_or_blank_access_falls_back_safely() {
        assert_eq!(
            resolve_registry_link_url(
                "https://app.codemod.com",
                Some("@acme/secret"),
                Some("enterprise")
            ),
            "https://app.codemod.com/registry"
        );
        assert_eq!(
            resolve_registry_link_url("https://app.codemod.com", Some("debarrel"), Some("  ")),
            "https://app.codemod.com/registry/debarrel"
        );
    }

    #[test]
    fn trailing_slash_on_base_is_normalized() {
        assert_eq!(
            resolve_registry_link_url("https://app.codemod.com/", Some("debarrel"), Some("public")),
            "https://app.codemod.com/registry/debarrel"
        );
    }

    #[test]
    fn resolve_never_returns_empty() {
        let cases = [
            ("", None, None),
            ("", Some(""), Some("")),
            (" ", Some(" "), Some(" private ")),
        ];

        for (base, path, access) in cases {
            let url = resolve_registry_link_url(base, path, access);
            assert!(!url.trim().is_empty(), "empty URL for {base:?}/{path:?}");
            assert!(
                url.starts_with("http://") || url.starts_with("https://"),
                "non-absolute URL: {url}"
            );
        }
    }
}
