fn main() {
    println!("cargo:rerun-if-env-changed=POSTHOG_API_KEY");
    let api_key = std::env::var("POSTHOG_API_KEY").unwrap_or_else(|_| "".to_string());
    println!("cargo:rustc-env=POSTHOG_API_KEY={api_key}");

    println!("cargo:rerun-if-env-changed=SCARF_GATEWAY_URL");
    let scarf_gateway_url = std::env::var("SCARF_GATEWAY_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
        .unwrap_or_else(|| "https://codemod.gateway.scarf.sh/telemetry".to_string());
    println!("cargo:rustc-env=SCARF_GATEWAY_URL={scarf_gateway_url}");
}
