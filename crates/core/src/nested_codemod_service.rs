use butterflow_models::{Error, Result, Workflow};
use log::warn;

use crate::{
    engine::CodemodDependency,
    registry::{RegistryClient, ResolvedPackage},
};

pub(crate) struct ResolvedNestedCodemod {
    pub(crate) package: ResolvedPackage,
    pub(crate) workflow: Workflow,
    pub(crate) dependency_chain: Vec<CodemodDependency>,
}

pub(crate) struct NestedCodemodService<'a> {
    registry_client: &'a RegistryClient,
}

impl<'a> NestedCodemodService<'a> {
    pub(crate) fn new(registry_client: &'a RegistryClient) -> Self {
        Self { registry_client }
    }

    pub(crate) fn find_cycle_in_chain(
        source: &str,
        dependency_chain: &[CodemodDependency],
    ) -> Option<String> {
        dependency_chain
            .iter()
            .find(|dep| dep.source == source)
            .map(|dep| dep.source.clone())
    }

    pub(crate) fn format_cycle_error(
        prefix: &str,
        source: &str,
        dependency_chain: &[CodemodDependency],
        validation_message: &str,
    ) -> Error {
        let cycle_start =
            Self::find_cycle_in_chain(source, dependency_chain).unwrap_or_else(|| source.into());
        let chain_str = dependency_chain
            .iter()
            .map(|d| d.source.as_str())
            .collect::<Vec<_>>()
            .join(" → ");

        Error::Other(format!(
            "{prefix}\n\
            Cycle: {} → {} → {}\n\
            {validation_message}\n\
            Please review your codemod dependencies to remove the circular reference.",
            cycle_start,
            if chain_str.is_empty() {
                "(root)"
            } else {
                &chain_str
            },
            source
        ))
    }

    pub(crate) async fn resolve(
        &self,
        source: &str,
        dependency_chain: &[CodemodDependency],
    ) -> Result<ResolvedNestedCodemod> {
        if Self::find_cycle_in_chain(source, dependency_chain).is_some() {
            return Err(Self::format_cycle_error(
                "Runtime codemod dependency cycle detected!",
                source,
                dependency_chain,
                "This cycle was not caught during validation, indicating a dynamic dependency.",
            ));
        }

        let package = self
            .registry_client
            .resolve_package(source, None, false, None)
            .await
            .map_err(|e| Error::Other(format!("Failed to resolve package: {e}")))?;
        let workflow = Self::load_workflow(&package)?;
        let dependency_chain = Self::extend_dependency_chain(source, dependency_chain);

        Ok(ResolvedNestedCodemod {
            package,
            workflow,
            dependency_chain,
        })
    }

    pub(crate) async fn validate_workflow_dependencies(
        &self,
        workflow: &Workflow,
        dependency_chain: &[CodemodDependency],
    ) -> Result<()> {
        for node in &workflow.nodes {
            for step in &node.steps {
                if let butterflow_models::step::StepAction::Codemod(codemod) = &step.action {
                    if Self::find_cycle_in_chain(&codemod.source, dependency_chain).is_some() {
                        return Err(Self::format_cycle_error(
                            "Codemod dependency cycle detected!",
                            &codemod.source,
                            dependency_chain,
                            "This would cause infinite recursion during execution.",
                        ));
                    }

                    if let Err(error) = self
                        .resolve_and_validate(&codemod.source, dependency_chain)
                        .await
                    {
                        warn!(
                            "Failed to validate codemod dependency {}: {}",
                            codemod.source, error
                        );
                    }
                }
            }
        }

        Ok(())
    }

    async fn resolve_and_validate(
        &self,
        source: &str,
        dependency_chain: &[CodemodDependency],
    ) -> Result<()> {
        let package = self
            .registry_client
            .resolve_package(source, None, false, None)
            .await
            .map_err(|e| Error::Other(format!("Failed to resolve codemod {source}: {e}")))?;
        let workflow = Self::load_workflow(&package)?;
        let dependency_chain = Self::extend_dependency_chain(source, dependency_chain);

        Box::pin(self.validate_workflow_dependencies(&workflow, &dependency_chain)).await
    }

    fn load_workflow(package: &ResolvedPackage) -> Result<Workflow> {
        let workflow_path = package.package_dir.join("workflow.yaml");
        if !workflow_path.exists() {
            return Err(Error::Other(format!(
                "Workflow file not found in codemod package: {}",
                workflow_path.display()
            )));
        }

        let workflow_content = std::fs::read_to_string(&workflow_path)
            .map_err(|e| Error::Other(format!("Failed to read workflow file: {e}")))?;

        serde_yaml::from_str(&workflow_content)
            .map_err(|e| Error::Other(format!("Failed to parse workflow YAML: {e}")))
    }

    fn extend_dependency_chain(
        source: &str,
        dependency_chain: &[CodemodDependency],
    ) -> Vec<CodemodDependency> {
        let mut chain = dependency_chain.to_vec();
        chain.push(CodemodDependency {
            source: source.to_string(),
        });
        chain
    }
}
