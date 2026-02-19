use anyhow::{anyhow, bail, Result};
use clap::ValueEnum;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum Harness {
    #[default]
    Auto,
    Claude,
    Goose,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum OutputFormat {
    #[default]
    Table,
    Json,
    Yaml,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallScope {
    Project,
    User,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallRequest {
    pub scope: InstallScope,
    pub force: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstalledSkill {
    pub name: String,
    pub path: PathBuf,
    pub version: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub enum VerificationStatus {
    Pass,
    Fail,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerificationCheck {
    pub skill: String,
    pub status: VerificationStatus,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompatibilityMetadata {
    pub harness: Harness,
    pub supports_project_scope: bool,
    pub supports_user_scope: bool,
    pub supports_verify: bool,
}

pub trait HarnessAdapter: Send + Sync {
    fn metadata(&self) -> CompatibilityMetadata;
    fn install_skills(&self, request: &InstallRequest) -> Result<Vec<InstalledSkill>>;
    fn list_skills(&self) -> Result<Vec<InstalledSkill>>;
    fn verify_skills(&self) -> Result<Vec<VerificationCheck>>;
}

#[derive(Debug, Default)]
pub struct ClaudeHarnessAdapter;

impl HarnessAdapter for ClaudeHarnessAdapter {
    fn metadata(&self) -> CompatibilityMetadata {
        CompatibilityMetadata {
            harness: Harness::Claude,
            supports_project_scope: true,
            supports_user_scope: true,
            supports_verify: true,
        }
    }

    fn install_skills(&self, _request: &InstallRequest) -> Result<Vec<InstalledSkill>> {
        bail!("Claude harness install-skills is not implemented yet")
    }

    fn list_skills(&self) -> Result<Vec<InstalledSkill>> {
        bail!("Claude harness list-skills is not implemented yet")
    }

    fn verify_skills(&self) -> Result<Vec<VerificationCheck>> {
        bail!("Claude harness verify-skills is not implemented yet")
    }
}

#[derive(Debug, Default)]
pub struct GooseHarnessAdapter;

impl HarnessAdapter for GooseHarnessAdapter {
    fn metadata(&self) -> CompatibilityMetadata {
        CompatibilityMetadata {
            harness: Harness::Goose,
            supports_project_scope: true,
            supports_user_scope: true,
            supports_verify: true,
        }
    }

    fn install_skills(&self, _request: &InstallRequest) -> Result<Vec<InstalledSkill>> {
        bail!("Goose harness install-skills is not implemented yet")
    }

    fn list_skills(&self) -> Result<Vec<InstalledSkill>> {
        bail!("Goose harness list-skills is not implemented yet")
    }

    fn verify_skills(&self) -> Result<Vec<VerificationCheck>> {
        bail!("Goose harness verify-skills is not implemented yet")
    }
}

pub fn resolve_adapter(harness: Harness) -> Result<Box<dyn HarnessAdapter>> {
    match harness {
        Harness::Auto | Harness::Claude => Ok(Box::new(ClaudeHarnessAdapter)),
        Harness::Goose => Ok(Box::new(GooseHarnessAdapter)),
    }
}

pub fn resolve_install_scope(project: bool, user: bool) -> Result<InstallScope> {
    match (project, user) {
        (true, true) => Err(anyhow!(
            "Conflicting scope flags: use either --project or --user"
        )),
        (true, false) => Ok(InstallScope::Project),
        (false, true) => Ok(InstallScope::User),
        (false, false) => Ok(InstallScope::Project),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_adapter_returns_known_harnesses() {
        assert_eq!(
            resolve_adapter(Harness::Claude).unwrap().metadata().harness,
            Harness::Claude
        );
        assert_eq!(
            resolve_adapter(Harness::Goose).unwrap().metadata().harness,
            Harness::Goose
        );
    }

    #[test]
    fn resolve_install_scope_rejects_conflicting_flags() {
        assert!(resolve_install_scope(true, true).is_err());
    }
}
