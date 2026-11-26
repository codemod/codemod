pub mod oidc;
pub mod storage;
pub mod types;

/// Serialize status, not `StoredAuth`: the latter contains credential values.
pub(crate) fn status_json(registry: &str, auth: Option<&storage::StoredAuth>) -> serde_json::Value {
    serde_json::json!({
        "registry": registry,
        "logged_in": auth.is_some(),
        "user": auth.map(|auth| &auth.user),
    })
}

pub use oidc::OidcClient;
pub use storage::TokenStorage;
