use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AuthTokens {
    pub(crate) access_token: String,
    pub(crate) refresh_token: Option<String>,
    pub(crate) expires_at: Option<DateTime<Utc>>,
    pub(crate) scope: Vec<String>,
    pub(crate) token_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct UserInfo {
    pub(crate) id: String,
    pub(crate) username: String,
    pub(crate) email: String,
    pub(crate) organizations: Option<Vec<OrganizationMembership>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct OrganizationMembership {
    pub(crate) name: String,
    pub(crate) role: String,
}
