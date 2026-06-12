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

        Error::Other(format!(
            "{prefix}\n\
            Cycle detected while resolving codemod dependency \"{}\".\n\
            {validation_message}\n\
            This bundle cannot be executed because one of its nested codemod dependencies forms a cycle. \
            Contact the package owner or try a different bundle version.",
            cycle_start
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

    pub(crate) async fn find_dry_run_only_dependency(
        &self,
        workflow: &Workflow,
        dependency_chain: &[CodemodDependency],
    ) -> Result<Option<String>> {
        for node in &workflow.nodes {
            for step in &node.steps {
                if let butterflow_models::step::StepAction::Codemod(codemod) = &step.action {
                    let resolved = match self.resolve(&codemod.source, dependency_chain).await {
                        Ok(resolved) => resolved,
                        Err(error) => {
                            warn!(
                                "Failed to inspect codemod dependency {} for dry-run-only status: {}",
                                codemod.source, error
                            );
                            continue;
                        }
                    };

                    if resolved.package.dry_run_only {
                        return Ok(Some(codemod.source.clone()));
                    }

                    if let Some(source) = Box::pin(self.find_dry_run_only_dependency(
                        &resolved.workflow,
                        &resolved.dependency_chain,
                    ))
                    .await?
                    {
                        return Ok(Some(source));
                    }
                }
            }
        }

        Ok(None)
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

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::GzEncoder, Compression};
    use std::{collections::HashMap, io::Write, sync::Arc};
    use tokio::net::TcpListener;

    #[test]
    fn format_cycle_error_does_not_expose_dependency_chain() {
        let error = NestedCodemodService::format_cycle_error(
            "Runtime codemod dependency cycle detected!",
            "public-child",
            &[
                CodemodDependency {
                    source: "public-child".to_string(),
                },
                CodemodDependency {
                    source: "private-child".to_string(),
                },
            ],
            "This cycle was not caught during validation, indicating a dynamic dependency.",
        );
        let message = error.to_string();

        assert!(message.contains("Runtime codemod dependency cycle detected!"));
        assert!(message.contains("Cycle detected while resolving codemod dependency"));
        assert!(message.contains(
            "This bundle cannot be executed because one of its nested codemod dependencies forms a cycle."
        ));
        assert!(!message.contains("Please review your codemod dependencies"));
        assert!(message.contains("public-child"));
        assert!(!message.contains("private-child"));
        assert!(!message.contains("→"));
    }

    #[tokio::test]
    async fn find_dry_run_only_dependency_detects_nested_registry_child() {
        let packages = HashMap::from([
            (
                "child-public".to_string(),
                MockPackage {
                    workflow: workflow_yaml_with_child("child-pro"),
                    dry_run_only: false,
                },
            ),
            (
                "child-pro".to_string(),
                MockPackage {
                    workflow: workflow_yaml_without_children(),
                    dry_run_only: true,
                },
            ),
        ]);
        let (registry_url, server) = spawn_registry_server(packages).await;
        let cache_dir = tempfile::tempdir().expect("cache dir");
        let registry_client = RegistryClient::new(
            crate::registry::RegistryConfig {
                default_registry: registry_url,
                cache_dir: cache_dir.path().to_path_buf(),
            },
            None,
        );
        let workflow: Workflow =
            serde_yaml::from_str(&workflow_yaml_with_child("child-public")).expect("workflow yaml");

        let dry_run_only_dependency = NestedCodemodService::new(&registry_client)
            .find_dry_run_only_dependency(&workflow, &[])
            .await
            .expect("dry-run-only scan should succeed");

        assert_eq!(dry_run_only_dependency.as_deref(), Some("child-pro"));
        server.abort();
    }

    #[tokio::test]
    async fn find_dry_run_only_dependency_skips_unresolved_children() {
        let packages = HashMap::from([(
            "child-pro".to_string(),
            MockPackage {
                workflow: workflow_yaml_without_children(),
                dry_run_only: true,
            },
        )]);
        let (registry_url, server) = spawn_registry_server(packages).await;
        let cache_dir = tempfile::tempdir().expect("cache dir");
        let registry_client = RegistryClient::new(
            crate::registry::RegistryConfig {
                default_registry: registry_url,
                cache_dir: cache_dir.path().to_path_buf(),
            },
            None,
        );
        let workflow: Workflow = serde_yaml::from_str(&workflow_yaml_with_children(&[
            "missing-child",
            "child-pro",
        ]))
        .expect("workflow yaml");

        let dry_run_only_dependency = NestedCodemodService::new(&registry_client)
            .find_dry_run_only_dependency(&workflow, &[])
            .await
            .expect("dry-run-only scan should skip unresolved children");

        assert_eq!(dry_run_only_dependency.as_deref(), Some("child-pro"));
        server.abort();
    }

    #[derive(Clone)]
    struct MockPackage {
        workflow: String,
        dry_run_only: bool,
    }

    async fn spawn_registry_server(
        packages: HashMap<String, MockPackage>,
    ) -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test registry");
        let addr = listener.local_addr().expect("test registry address");
        let registry_url = format!("http://{addr}");
        let packages = Arc::new(packages);
        let server_url = registry_url.clone();

        let handle = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };

                let packages = Arc::clone(&packages);
                let server_url = server_url.clone();
                tokio::spawn(async move {
                    let mut buffer = [0; 4096];
                    let Ok(size) = stream
                        .readable()
                        .await
                        .and_then(|_| stream.try_read(&mut buffer))
                    else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&buffer[..size]);
                    let Some(request_line) = request.lines().next() else {
                        return;
                    };
                    let mut parts = request_line.split_whitespace();
                    let method = parts.next().unwrap_or_default();
                    let path = parts.next().unwrap_or_default();

                    let (status, content_type, body) =
                        registry_response(method, path, &server_url, &packages).unwrap_or_else(
                            || ("404 Not Found", "text/plain", b"not found".to_vec()),
                        );

                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );

                    let _ = stream.writable().await;
                    let _ = stream.try_write(response.as_bytes());
                    if method != "HEAD" && !body.is_empty() {
                        let _ = stream.writable().await;
                        let _ = stream.try_write(&body);
                    }
                });
            }
        });

        (registry_url, handle)
    }

    fn registry_response(
        method: &str,
        path: &str,
        registry_url: &str,
        packages: &HashMap<String, MockPackage>,
    ) -> Option<(&'static str, &'static str, Vec<u8>)> {
        if let Some(package_name) = path.strip_prefix("/cdn/") {
            let package_name = package_name.strip_suffix(".tgz")?;
            let package = packages.get(package_name)?;
            return match method {
                "HEAD" => Some(("200 OK", "application/gzip", Vec::new())),
                "GET" => Some((
                    "200 OK",
                    "application/gzip",
                    package_archive(&package.workflow),
                )),
                _ => None,
            };
        }

        let package_name = path.strip_prefix("/api/v1/registry/packages/")?;

        if method == "GET" && !package_name.contains("/download/") {
            packages.get(package_name)?;
            return Some((
                "200 OK",
                "application/json",
                package_info_response(package_name).into_bytes(),
            ));
        }

        if let Some((package_name, version)) = package_name.split_once("/download/") {
            let package = packages.get(package_name)?;
            if version != "1.0.0" {
                return None;
            }

            return match method {
                "HEAD" => Some(("200 OK", "application/json", Vec::new())),
                "GET" => Some((
                    "200 OK",
                    "application/json",
                    download_response(registry_url, package_name, package.dry_run_only)
                        .into_bytes(),
                )),
                _ => None,
            };
        }

        None
    }

    fn package_info_response(package_name: &str) -> String {
        serde_json::json!({
            "id": format!("pkg_{package_name}"),
            "name": package_name,
            "scope": null,
            "is_legacy": false,
            "latest_version": "1.0.0",
            "versions": {
                "1.0.0": {
                    "version": "1.0.0",
                    "description": null,
                    "checksum": "sha256:test",
                    "size": 1
                }
            }
        })
        .to_string()
    }

    fn download_response(registry_url: &str, package_name: &str, dry_run_only: bool) -> String {
        serde_json::json!({
            "download_url": format!("{registry_url}/cdn/{package_name}.tgz"),
            "expires_at": "2099-01-01T00:00:00Z",
            "dry_run_only": dry_run_only
        })
        .to_string()
    }

    fn package_archive(workflow: &str) -> Vec<u8> {
        let mut tar_buffer = Vec::new();
        {
            let mut archive = tar::Builder::new(&mut tar_buffer);
            append_tar_file(&mut archive, "codemod.yaml", b"name: test\n");
            append_tar_file(&mut archive, "workflow.yaml", workflow.as_bytes());
            archive.finish().expect("finish tar");
        }

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar_buffer).expect("write gzip");
        encoder.finish().expect("finish gzip")
    }

    fn append_tar_file(archive: &mut tar::Builder<&mut Vec<u8>>, path: &str, contents: &[u8]) {
        let mut header = tar::Header::new_gnu();
        header.set_path(path).expect("set tar path");
        header.set_size(contents.len() as u64);
        header.set_cksum();
        archive.append(&header, contents).expect("append tar file");
    }

    fn workflow_yaml_with_child(source: &str) -> String {
        format!(
            "version: '1'\nnodes:\n- id: test\n  name: Test\n  steps:\n  - name: Run child\n    codemod:\n      source: {source}\n"
        )
    }

    fn workflow_yaml_with_children(sources: &[&str]) -> String {
        let steps = sources
            .iter()
            .map(|source| format!("  - name: Run {source}\n    codemod:\n      source: {source}\n"))
            .collect::<String>();

        format!("version: '1'\nnodes:\n- id: test\n  name: Test\n  steps:\n{steps}")
    }

    fn workflow_yaml_without_children() -> String {
        "version: '1'\nnodes:\n- id: test\n  name: Test\n  steps:\n  - name: Done\n    run: echo done\n"
            .to_string()
    }
}
