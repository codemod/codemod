use butterflow_core::registry::ResolvedPackage;
use butterflow_core::registry_link::resolve_registry_link_url;

pub fn registry_link_url_for_resolved_package(
    default_registry_url: &str,
    resolved_package: &ResolvedPackage,
) -> String {
    if let Some(metadata) = &resolved_package.registry_metadata {
        return resolve_registry_link_url(
            &metadata.registry_base_url,
            Some(metadata.package_web_path.as_str()),
            metadata.access.as_deref(),
        );
    }

    resolve_registry_link_url(default_registry_url, None, None)
}

pub fn registry_link_url_for_local_run(default_registry_url: &str) -> String {
    resolve_registry_link_url(default_registry_url, None, None)
}
