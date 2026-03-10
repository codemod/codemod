use butterflow_core::registry::{
    AuthProvider, AuthTokens, RegistryAuth, RegistryError, Result as RegistryResult,
};

use crate::auth::TokenStorage;

pub struct CliAuthProvider {
    pub storage: TokenStorage,
}

impl CliAuthProvider {
    pub fn from_storage(storage: TokenStorage) -> Self {
        Self { storage }
    }
}

impl AuthProvider for CliAuthProvider {
    fn get_auth_for_registry(&self, registry_url: &str) -> RegistryResult<Option<RegistryAuth>> {
        match self.storage.get_auth_for_registry(registry_url) {
            Ok(Some(auth)) => Ok(Some(RegistryAuth {
                tokens: AuthTokens {
                    access_token: auth.tokens.access_token,
                    refresh_token: auth.tokens.refresh_token,
                },
            })),
            Ok(None) => Ok(None),
            Err(e) => Err(RegistryError::AuthProviderError(e.into())),
        }
    }
}
