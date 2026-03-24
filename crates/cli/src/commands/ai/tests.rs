use super::update::auto_safe::extract_skill_archive_writes;
use super::update::auto_safe::{apply_staged_component_updates, maybe_apply_auto_safe_updates};
use super::update::output::{
    build_install_output, build_update_policy_output, BuildInstallOutputInput,
};
use super::update::policy::{
    parse_update_remote_source_value, remote_manifest_endpoint, resolve_update_policy_context,
    validate_remote_update_manifest, UpdatePolicyResolveOptions,
};
use super::update::reconcile::{
    build_component_reconcile_decisions, update_policy_runtime_message,
};
use super::update::types::{
    AutoSafeApplyResult, AutoSafeComponentStatus, ManagedUpdateManifest,
    ManagedUpdateManifestComponent, ReconcileDecisionStatus, RemoteManifestSnapshot,
    StagedComponentUpdate, StagedFileWrite, UpdatePolicyContext, UpdatePolicyMode,
    MANAGED_UPDATE_MANIFEST_PUBLIC_KEYS_ENV_VAR, MANAGED_UPDATE_MANIFEST_SIGNATURES_HEADER,
    MANAGED_UPDATE_POLICY_LOCAL_SOURCE,
};
use super::{
    goose_project_scope_command_warning, interactive_user_scope_label,
    managed_components_from_install, scope_prompt_options,
};
use crate::commands::harness_adapter::{
    Harness, InstallScope, InstalledSkill, ManagedComponentKind, ManagedComponentSnapshot,
    ManagedStateWriteResult,
};
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use hyper::header::HeaderName;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server, StatusCode};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use tokio::runtime::Runtime;
use tokio::sync::oneshot;

static ENV_GUARD: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[derive(Clone)]
struct TestHttpFixture {
    status: StatusCode,
    body: Vec<u8>,
    headers: Vec<(String, String)>,
}

struct TestHttpServer {
    base_url: String,
    shutdown: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl TestHttpServer {
    async fn start_with_builder<F>(build_routes: F) -> Self
    where
        F: FnOnce(&str) -> HashMap<String, TestHttpFixture>,
    {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("expected local listener bind");
        listener
            .set_nonblocking(true)
            .expect("expected non-blocking listener");
        let local_addr = listener.local_addr().expect("expected listener address");
        let base_url = format!("http://{local_addr}");
        let routes = Arc::new(build_routes(&base_url));

        let make_service = make_service_fn(move |_| {
            let routes = Arc::clone(&routes);
            async move {
                Ok::<_, std::convert::Infallible>(service_fn(move |request: Request<Body>| {
                    let routes = Arc::clone(&routes);
                    async move {
                        if request.method() != Method::GET {
                            let response = Response::builder()
                                .status(StatusCode::METHOD_NOT_ALLOWED)
                                .body(Body::from("method not allowed"))
                                .expect("expected method not allowed response");
                            return Ok::<_, std::convert::Infallible>(response);
                        }

                        let path = request.uri().path().to_string();
                        if let Some(fixture) = routes.get(&path) {
                            let mut builder = Response::builder().status(fixture.status);
                            for (name, value) in &fixture.headers {
                                if let Ok(header_name) = HeaderName::from_bytes(name.as_bytes()) {
                                    builder = builder.header(header_name, value);
                                }
                            }
                            let response = builder
                                .body(Body::from(fixture.body.clone()))
                                .expect("expected fixture response");
                            return Ok::<_, std::convert::Infallible>(response);
                        }

                        let response = Response::builder()
                            .status(StatusCode::NOT_FOUND)
                            .body(Body::from("not found"))
                            .expect("expected not found response");
                        Ok::<_, std::convert::Infallible>(response)
                    }
                }))
            }
        });

        let server = Server::from_tcp(listener)
            .expect("expected server from listener")
            .serve(make_service);
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _ = server
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await;
        });

        Self {
            base_url,
            shutdown: Some(shutdown_tx),
            task,
        }
    }

    async fn shutdown(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let _ = self.task.await;
    }
}

struct EnvRestoreGuard {
    saved: Vec<(&'static str, Option<String>)>,
}

impl EnvRestoreGuard {
    fn set(vars: &[(&'static str, String)]) -> Self {
        let mut saved = Vec::with_capacity(vars.len());
        for (key, value) in vars {
            saved.push((*key, std::env::var(key).ok()));
            std::env::set_var(key, value);
        }
        Self { saved }
    }
}

impl Drop for EnvRestoreGuard {
    fn drop(&mut self) {
        for (key, previous_value) in &self.saved {
            match previous_value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

fn signing_key_fixture() -> SigningKey {
    SigningKey::from_bytes(&[7_u8; 32])
}

fn manifest_public_keys_env_value(public_key_base64: &str) -> String {
    format!(r#"{{"test":"{public_key_base64}"}}"#)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn harness_skill_path(root: &std::path::Path, harness: Harness) -> PathBuf {
    match harness {
        Harness::Claude => root.join(".claude/skills/codemod/SKILL.md"),
        Harness::Goose => root.join(".goose/skills/codemod/SKILL.md"),
        Harness::Opencode => root.join(".opencode/skills/codemod/SKILL.md"),
        Harness::Cursor => root.join(".cursor/skills/codemod/SKILL.md"),
        Harness::Codex | Harness::Antigravity => root.join(".agents/skills/codemod/SKILL.md"),
        Harness::Auto => root.join(".claude/skills/codemod/SKILL.md"),
    }
}

#[test]
fn interactive_user_scope_label_defaults_auto_to_claude_path() {
    assert_eq!(
        interactive_user_scope_label(Harness::Auto),
        "user (claude: ~/.claude/skills)"
    );
}

#[test]
fn interactive_user_scope_label_uses_explicit_harness_path() {
    assert_eq!(
        interactive_user_scope_label(Harness::Opencode),
        "user (opencode: ~/.opencode/skills)"
    );
}

#[test]
fn interactive_user_scope_label_uses_goose_command_config_hint() {
    assert_eq!(
        interactive_user_scope_label(Harness::Goose),
        "user (goose: ~/.goose/skills + ~/.config/goose/config.yaml)"
    );
}

#[test]
fn goose_scope_prompt_defaults_to_user_with_command_explanation() {
    let (options, starting_cursor) = scope_prompt_options(Harness::Goose);
    assert_eq!(starting_cursor, 0);
    assert_eq!(options[0].scope, InstallScope::User);
    assert!(options[0].label.contains("/codemod"));
    assert!(options[0].label.contains("recommended"));
    assert_eq!(options[1].scope, InstallScope::Project);
    assert!(options[1].label.contains("skills only"));
}

#[test]
fn goose_project_scope_warning_is_explicit() {
    let warning = goose_project_scope_command_warning(Harness::Goose, InstallScope::Project)
        .expect("expected goose project warning");
    assert!(warning.contains("/codemod"));
    assert!(warning.contains("project scope installed skills only"));
    assert!(warning.contains("--user"));
    assert!(goose_project_scope_command_warning(Harness::Goose, InstallScope::User).is_none());
}

#[test]
fn install_output_json_includes_codemod_mcp_entry() {
    let update_policy = UpdatePolicyContext {
        mode: UpdatePolicyMode::Manual,
        remote_source: MANAGED_UPDATE_POLICY_LOCAL_SOURCE.to_string(),
        fallback_applied: false,
        remote_manifest: None,
        warnings: Vec::new(),
    };
    let installed = vec![
        InstalledSkill {
            name: "codemod".to_string(),
            path: PathBuf::from("/tmp/.claude/skills/codemod/SKILL.md"),
            version: Some("1.0.0".to_string()),
            scope: Some(InstallScope::Project),
        },
        InstalledSkill {
            name: "codemod-mcp".to_string(),
            path: PathBuf::from("/tmp/.mcp.json"),
            version: None,
            scope: Some(InstallScope::Project),
        },
    ];
    let managed_components = managed_components_from_install(&installed, &[], &[], &[]);
    let component_decisions =
        build_component_reconcile_decisions(&update_policy, Harness::Claude, &managed_components);
    let output = build_install_output(BuildInstallOutputInput {
        harness: Harness::Claude,
        scope: InstallScope::Project,
        installed,
        managed_state: Some(ManagedStateWriteResult {
            path: PathBuf::from("/tmp/.claude/codemod/managed-install-state.json"),
            status: crate::commands::harness_adapter::ManagedStateWriteStatus::Created,
        }),
        update_policy: &update_policy,
        component_decisions,
        auto_safe_apply: None,
        notes: Vec::new(),
        warnings: Vec::new(),
        restart_hint: Some("Restart now".to_string()),
    });

    let output_json = serde_json::to_value(&output).expect("install output should serialize");
    let installed = output_json
        .get("installed")
        .and_then(Value::as_array)
        .expect("installed should be an array");

    let codemod_mcp = installed
        .iter()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some("codemod-mcp"))
        .expect("expected codemod-mcp installed entry");

    assert_eq!(output_json.get("ok").and_then(Value::as_bool), Some(true));
    assert_eq!(
        output_json.get("harness").and_then(Value::as_str),
        Some("claude")
    );
    assert_eq!(
        output_json.get("scope").and_then(Value::as_str),
        Some("project")
    );
    assert_eq!(
        output_json
            .get("managed_state")
            .and_then(|value| value.get("status"))
            .and_then(Value::as_str),
        Some("created")
    );
    assert_eq!(
        output_json
            .get("update_policy")
            .and_then(|value| value.get("mode"))
            .and_then(Value::as_str),
        Some("manual")
    );
    assert_eq!(
        output_json
            .get("update_policy")
            .and_then(|value| value.get("behavior"))
            .and_then(Value::as_str),
        Some("reconcile_on_install")
    );
    assert_eq!(
        output_json
            .get("update_policy")
            .and_then(|value| value.get("remote_source"))
            .and_then(Value::as_str),
        Some("local_embedded_only")
    );
    assert_eq!(
        output_json
            .get("update_policy")
            .and_then(|value| value.get("fallback_applied"))
            .and_then(Value::as_bool),
        Some(false)
    );
    assert_eq!(
        output_json
            .get("update_policy")
            .and_then(|value| value.get("trigger"))
            .and_then(Value::as_str),
        Some("install_and_periodic")
    );
    assert_eq!(
        output_json.get("restart_hint").and_then(Value::as_str),
        Some("Restart now")
    );
    assert!(codemod_mcp.get("path").and_then(Value::as_str).is_some());
    assert_eq!(
        codemod_mcp.get("version").and_then(Value::as_str),
        Some("latest")
    );
}

#[test]
fn managed_components_include_discovery_guides_and_mcp_kind() {
    let installed = vec![
        InstalledSkill {
            name: "codemod".to_string(),
            path: PathBuf::from("/tmp/.claude/skills/codemod/SKILL.md"),
            version: Some("1.0.0".to_string()),
            scope: Some(InstallScope::Project),
        },
        InstalledSkill {
            name: "codemod-mcp".to_string(),
            path: PathBuf::from("/tmp/.mcp.json"),
            version: None,
            scope: Some(InstallScope::Project),
        },
    ];
    let discovery_paths = vec![
        PathBuf::from("/tmp/AGENTS.md"),
        PathBuf::from("/tmp/CLAUDE.md"),
    ];

    let periodic_trigger_paths = vec![PathBuf::from(
        "/tmp/.claude/codemod/periodic-update/check-updates.sh",
    )];
    let command_paths = vec![PathBuf::from("/tmp/.claude/commands/codemod.md")];
    let components = managed_components_from_install(
        &installed,
        &command_paths,
        &discovery_paths,
        &periodic_trigger_paths,
    );
    assert_eq!(components.len(), 6);

    let mcp_component = components
        .iter()
        .find(|component| component.id == "codemod-mcp")
        .expect("expected codemod-mcp managed component");
    assert_eq!(mcp_component.kind, ManagedComponentKind::McpConfig);

    let discovery_component = components
        .iter()
        .find(|component| component.id == "discovery-guide:AGENTS.md")
        .expect("expected AGENTS.md discovery component");
    assert_eq!(
        discovery_component.kind,
        ManagedComponentKind::DiscoveryGuide
    );

    let periodic_component = components
        .iter()
        .find(|component| component.id == "periodic-trigger:check-updates.sh")
        .expect("expected periodic trigger component");
    assert_eq!(
        periodic_component.kind,
        ManagedComponentKind::DiscoveryGuide
    );

    let command_component = components
        .iter()
        .find(|component| component.id == "command:codemod")
        .expect("expected codemod command component");
    assert_eq!(command_component.kind, ManagedComponentKind::Command);
}

#[test]
fn update_policy_runtime_message_notify_reflects_state_status() {
    let updated_state = ManagedStateWriteResult {
        path: PathBuf::from("/tmp/managed.json"),
        status: crate::commands::harness_adapter::ManagedStateWriteStatus::Updated,
    };
    let unchanged_state = ManagedStateWriteResult {
        path: PathBuf::from("/tmp/managed.json"),
        status: crate::commands::harness_adapter::ManagedStateWriteStatus::Unchanged,
    };
    let notify_context = UpdatePolicyContext {
        mode: UpdatePolicyMode::Notify,
        remote_source: "registry:https://app.codemod.com/".to_string(),
        fallback_applied: true,
        remote_manifest: None,
        warnings: Vec::new(),
    };
    let autosafe_context = UpdatePolicyContext {
        mode: UpdatePolicyMode::AutoSafe,
        remote_source: "registry:https://app.codemod.com/".to_string(),
        fallback_applied: true,
        remote_manifest: None,
        warnings: Vec::new(),
    };
    let manual_context = UpdatePolicyContext {
        mode: UpdatePolicyMode::Manual,
        remote_source: MANAGED_UPDATE_POLICY_LOCAL_SOURCE.to_string(),
        fallback_applied: false,
        remote_manifest: None,
        warnings: Vec::new(),
    };

    assert!(
        update_policy_runtime_message(&notify_context, Some(&updated_state), None)
            .unwrap()
            .contains("local fallback")
    );
    assert!(
        update_policy_runtime_message(&notify_context, Some(&unchanged_state), None)
            .unwrap()
            .contains("no codemod-managed state change")
    );
    assert!(update_policy_runtime_message(
        &autosafe_context,
        None,
        Some(&AutoSafeApplyResult {
            attempted: 2,
            applied: 1,
            skipped: 1,
            failed: 0,
            rolled_back: false,
            rollback_reason: None,
            components: Vec::new(),
        }),
    )
    .unwrap()
    .contains("attempted 2"));
    assert!(update_policy_runtime_message(
        &autosafe_context,
        None,
        Some(&AutoSafeApplyResult {
            attempted: 0,
            applied: 0,
            skipped: 0,
            failed: 0,
            rolled_back: false,
            rollback_reason: Some("remote_manifest_unavailable".to_string()),
            components: Vec::new(),
        }),
    )
    .is_none());
    assert!(update_policy_runtime_message(&autosafe_context, None, None).is_none());
    assert!(update_policy_runtime_message(&manual_context, None, None).is_none());
}

#[test]
fn parse_update_remote_source_value_handles_local_registry_and_url() {
    let local_source =
        parse_update_remote_source_value("local").expect("local update source should resolve");
    assert_eq!(local_source, MANAGED_UPDATE_POLICY_LOCAL_SOURCE);

    let url_source = parse_update_remote_source_value("https://updates.codemod.com")
        .expect("absolute URL update source should resolve");
    assert_eq!(url_source, "url:https://updates.codemod.com/");

    match parse_update_remote_source_value("registry") {
        Ok(registry_source) => assert!(registry_source.starts_with("registry:")),
        Err(error) => assert!(error.contains("could not resolve registry update source")),
    }
}

#[test]
fn parse_update_remote_source_value_rejects_invalid_values_and_legacy_aliases() {
    let invalid =
        parse_update_remote_source_value("not a url").expect_err("invalid value must fail");
    assert!(invalid.contains("unsupported update source"));

    let empty = parse_update_remote_source_value("  ").expect_err("empty value must fail");
    assert!(empty.contains("cannot be empty"));

    let embedded =
        parse_update_remote_source_value("embedded").expect_err("legacy alias must fail");
    assert!(embedded.contains("unsupported update source"));

    let registry_prefix = parse_update_remote_source_value("registry:https://example.com")
        .expect_err("legacy registry prefix must fail");
    assert!(registry_prefix.contains("unsupported update source"));

    let url_prefix = parse_update_remote_source_value("url:https://example.com/manifest.json")
        .expect_err("legacy url prefix must fail");
    assert!(url_prefix.contains("unsupported update source"));
}

#[test]
fn remote_manifest_endpoint_builds_registry_and_url_sources() {
    let registry_endpoint = remote_manifest_endpoint("registry:https://app.codemod.com/").unwrap();
    assert_eq!(
        registry_endpoint,
        "https://app.codemod.com/api/v1/ai/managed-components/manifest"
    );

    let url_endpoint =
        remote_manifest_endpoint("url:https://updates.codemod.com/manifest.json").unwrap();
    assert_eq!(url_endpoint, "https://updates.codemod.com/manifest.json");
}

#[test]
fn validate_remote_update_manifest_rejects_duplicate_component_ids() {
    let manifest = ManagedUpdateManifest {
        schema_version: "1".to_string(),
        generated_at: None,
        components: vec![
            ManagedUpdateManifestComponent {
                id: "codemod".to_string(),
                kind: "skill".to_string(),
                version: "1.0.0".to_string(),
                checksum_sha256: "d8b538f9f4a4e4f8d2832de45ffac4f8df2cd1bd4fd6ca1672b353d7dbdb3a92"
                    .to_string(),
                source_url: "https://updates.codemod.com/codemod.tar.gz".to_string(),
                min_cli_version: None,
                max_cli_version: None,
                harnesses: None,
            },
            ManagedUpdateManifestComponent {
                id: "codemod".to_string(),
                kind: "skill".to_string(),
                version: "1.0.1".to_string(),
                checksum_sha256: "b8b538f9f4a4e4f8d2832de45ffac4f8df2cd1bd4fd6ca1672b353d7dbdb3a92"
                    .to_string(),
                source_url: "https://updates.codemod.com/codemod-v101.tar.gz".to_string(),
                min_cli_version: None,
                max_cli_version: None,
                harnesses: None,
            },
        ],
    };

    let error = validate_remote_update_manifest(&manifest).unwrap_err();
    assert!(error.contains("duplicate component id"));
}

#[test]
fn build_update_policy_output_includes_remote_manifest_when_available() {
    let context = UpdatePolicyContext {
        mode: UpdatePolicyMode::Notify,
        remote_source: "registry:https://app.codemod.com/".to_string(),
        fallback_applied: true,
        remote_manifest: Some(RemoteManifestSnapshot {
            source: "https://app.codemod.com/api/v1/ai/managed-components/manifest".to_string(),
            authenticity_verified: true,
            manifest: ManagedUpdateManifest {
                schema_version: "1".to_string(),
                generated_at: None,
                components: vec![
                    ManagedUpdateManifestComponent {
                        id: "codemod".to_string(),
                        kind: "skill".to_string(),
                        version: "1.1.0".to_string(),
                        checksum_sha256:
                            "d8b538f9f4a4e4f8d2832de45ffac4f8df2cd1bd4fd6ca1672b353d7dbdb3a92"
                                .to_string(),
                        source_url: "https://updates.codemod.com/codemod.tar.gz".to_string(),
                        min_cli_version: None,
                        max_cli_version: None,
                        harnesses: None,
                    },
                    ManagedUpdateManifestComponent {
                        id: "codemod-mcp".to_string(),
                        kind: "mcp_config".to_string(),
                        version: "1.0.0".to_string(),
                        checksum_sha256:
                            "b8b538f9f4a4e4f8d2832de45ffac4f8df2cd1bd4fd6ca1672b353d7dbdb3a92"
                                .to_string(),
                        source_url: "https://updates.codemod.com/codemod-mcp.tar.gz".to_string(),
                        min_cli_version: None,
                        max_cli_version: None,
                        harnesses: Some(vec!["claude".to_string()]),
                    },
                    ManagedUpdateManifestComponent {
                        id: "codemod-extra".to_string(),
                        kind: "skill".to_string(),
                        version: "1.0.0".to_string(),
                        checksum_sha256:
                            "c8b538f9f4a4e4f8d2832de45ffac4f8df2cd1bd4fd6ca1672b353d7dbdb3a92"
                                .to_string(),
                        source_url: "https://updates.codemod.com/codemod-extra.tar.gz".to_string(),
                        min_cli_version: None,
                        max_cli_version: None,
                        harnesses: None,
                    },
                ],
            },
        }),
        warnings: Vec::new(),
    };

    let managed_components = vec![ManagedComponentSnapshot {
        id: "codemod".to_string(),
        kind: ManagedComponentKind::Skill,
        path: PathBuf::from("/tmp/.claude/skills/codemod/SKILL.md"),
        version: Some("1.0.0".to_string()),
    }];
    let component_decisions =
        build_component_reconcile_decisions(&context, Harness::Claude, &managed_components);
    let output = build_update_policy_output(&context, component_decisions, None);
    let manifest = output.remote_manifest.expect("expected remote manifest");
    assert_eq!(
        manifest.source,
        "https://app.codemod.com/api/v1/ai/managed-components/manifest"
    );
    assert_eq!(manifest.schema_version, "1");
    assert_eq!(manifest.component_count, 3);
    assert!(manifest.authenticity_verified);
    assert!(!output.component_decisions.is_empty());
}

#[test]
fn build_component_reconcile_decisions_classifies_statuses_with_reasons() {
    let context = UpdatePolicyContext {
        mode: UpdatePolicyMode::Notify,
        remote_source: "registry:https://app.codemod.com/".to_string(),
        fallback_applied: true,
        remote_manifest: Some(RemoteManifestSnapshot {
            source: "https://app.codemod.com/api/v1/ai/managed-components/manifest".to_string(),
            authenticity_verified: true,
            manifest: ManagedUpdateManifest {
                schema_version: "1".to_string(),
                generated_at: None,
                components: vec![
                    ManagedUpdateManifestComponent {
                        id: "codemod".to_string(),
                        kind: "skill".to_string(),
                        version: "1.1.0".to_string(),
                        checksum_sha256:
                            "d8b538f9f4a4e4f8d2832de45ffac4f8df2cd1bd4fd6ca1672b353d7dbdb3a92"
                                .to_string(),
                        source_url: "https://updates.codemod.com/codemod.tar.gz".to_string(),
                        min_cli_version: None,
                        max_cli_version: None,
                        harnesses: Some(vec!["claude".to_string()]),
                    },
                    ManagedUpdateManifestComponent {
                        id: "codemod-mcp".to_string(),
                        kind: "mcp_config".to_string(),
                        version: "1.0.0".to_string(),
                        checksum_sha256:
                            "b8b538f9f4a4e4f8d2832de45ffac4f8df2cd1bd4fd6ca1672b353d7dbdb3a92"
                                .to_string(),
                        source_url: "https://updates.codemod.com/codemod-mcp.tar.gz".to_string(),
                        min_cli_version: Some("999.0.0".to_string()),
                        max_cli_version: None,
                        harnesses: Some(vec!["claude".to_string()]),
                    },
                    ManagedUpdateManifestComponent {
                        id: "codemod-extra".to_string(),
                        kind: "skill".to_string(),
                        version: "1.0.0".to_string(),
                        checksum_sha256:
                            "c8b538f9f4a4e4f8d2832de45ffac4f8df2cd1bd4fd6ca1672b353d7dbdb3a92"
                                .to_string(),
                        source_url: "https://updates.codemod.com/codemod-extra.tar.gz".to_string(),
                        min_cli_version: None,
                        max_cli_version: None,
                        harnesses: None,
                    },
                ],
            },
        }),
        warnings: Vec::new(),
    };
    let local_components = vec![
        ManagedComponentSnapshot {
            id: "codemod".to_string(),
            kind: ManagedComponentKind::Skill,
            path: PathBuf::from("/tmp/.claude/skills/codemod/SKILL.md"),
            version: Some("1.0.0".to_string()),
        },
        ManagedComponentSnapshot {
            id: "codemod-mcp".to_string(),
            kind: ManagedComponentKind::McpConfig,
            path: PathBuf::from("/tmp/.mcp.json"),
            version: None,
        },
        ManagedComponentSnapshot {
            id: "discovery-guide:AGENTS.md".to_string(),
            kind: ManagedComponentKind::DiscoveryGuide,
            path: PathBuf::from("/tmp/AGENTS.md"),
            version: None,
        },
    ];

    let decisions =
        build_component_reconcile_decisions(&context, Harness::Claude, &local_components);
    assert_eq!(decisions.len(), 4);

    let cli_decision = decisions
        .iter()
        .find(|decision| decision.id == "codemod")
        .expect("expected codemod decision");
    assert_eq!(
        cli_decision.status,
        ReconcileDecisionStatus::UpdateAvailable
    );
    assert_eq!(cli_decision.reason, "remote_version_newer");

    let mcp_decision = decisions
        .iter()
        .find(|decision| decision.id == "codemod-mcp")
        .expect("expected codemod-mcp decision");
    assert_eq!(mcp_decision.status, ReconcileDecisionStatus::Incompatible);
    assert!(mcp_decision.reason.contains("cli_version_below_min"));

    let discovery_decision = decisions
        .iter()
        .find(|decision| decision.id == "discovery-guide:AGENTS.md")
        .expect("expected discovery guide decision");
    assert_eq!(
        discovery_decision.status,
        ReconcileDecisionStatus::Unverifiable
    );
    assert_eq!(discovery_decision.reason, "remote_component_missing");

    let remote_only_decision = decisions
        .iter()
        .find(|decision| decision.id == "codemod-extra")
        .expect("expected remote-only decision");
    assert_eq!(
        remote_only_decision.status,
        ReconcileDecisionStatus::UpdateAvailable
    );
    assert_eq!(remote_only_decision.reason, "component_missing_locally");
}

#[test]
fn build_component_reconcile_decisions_without_remote_manifest_are_unverifiable() {
    let context = UpdatePolicyContext {
        mode: UpdatePolicyMode::Notify,
        remote_source: MANAGED_UPDATE_POLICY_LOCAL_SOURCE.to_string(),
        fallback_applied: true,
        remote_manifest: None,
        warnings: Vec::new(),
    };
    let local_components = vec![ManagedComponentSnapshot {
        id: "codemod".to_string(),
        kind: ManagedComponentKind::Skill,
        path: PathBuf::from("/tmp/.claude/skills/codemod/SKILL.md"),
        version: Some("1.0.0".to_string()),
    }];

    let decisions =
        build_component_reconcile_decisions(&context, Harness::Claude, &local_components);
    assert_eq!(decisions.len(), 1);
    assert_eq!(decisions[0].status, ReconcileDecisionStatus::Unverifiable);
    assert_eq!(decisions[0].reason, "remote_manifest_unavailable");
}

#[test]
fn apply_staged_component_updates_rolls_back_on_write_failure() {
    let temp_dir = tempfile::tempdir().expect("expected temp dir");
    let target_file = temp_dir.path().join("component.txt");
    let target_dir = temp_dir.path().join("not-a-file");
    fs::write(&target_file, "before").expect("expected seed file write");
    fs::create_dir_all(&target_dir).expect("expected directory path");

    let result = apply_staged_component_updates(vec![
        StagedComponentUpdate {
            id: "component-a".to_string(),
            display_path: target_file.clone(),
            writes: vec![StagedFileWrite {
                path: target_file.clone(),
                bytes: b"after".to_vec(),
            }],
        },
        StagedComponentUpdate {
            id: "component-b".to_string(),
            display_path: target_dir.clone(),
            writes: vec![StagedFileWrite {
                path: target_dir.clone(),
                bytes: b"this-will-fail".to_vec(),
            }],
        },
    ]);

    assert!(result.rolled_back);
    assert!(result.failed > 0);
    assert!(result
        .components
        .iter()
        .any(|component| component.id == "component-a"
            && component.status == AutoSafeComponentStatus::RolledBack));
    assert_eq!(
        fs::read_to_string(&target_file).expect("expected restored component file"),
        "before"
    );
}

#[test]
fn extract_skill_archive_writes_supports_single_root_folder() {
    let temp_dir = tempfile::tempdir().expect("expected temp dir");
    let skill_root = temp_dir.path().join("codemod");
    let mut tar_bytes = Vec::new();

    {
        let gz_encoder =
            flate2::write::GzEncoder::new(&mut tar_bytes, flate2::Compression::default());
        let mut builder = tar::Builder::new(gz_encoder);

        let mut skill_header = tar::Header::new_gnu();
        let skill_content = b"---\nname: codemod\n---\n";
        skill_header.set_size(skill_content.len() as u64);
        skill_header.set_mode(0o644);
        skill_header.set_cksum();
        builder
            .append_data(&mut skill_header, "codemod/SKILL.md", &skill_content[..])
            .expect("expected skill entry");

        let mut recipe_header = tar::Header::new_gnu();
        let recipe_content = b"# Recipes\n";
        recipe_header.set_size(recipe_content.len() as u64);
        recipe_header.set_mode(0o644);
        recipe_header.set_cksum();
        builder
            .append_data(
                &mut recipe_header,
                "codemod/references/core/search-and-discovery.md",
                &recipe_content[..],
            )
            .expect("expected reference entry");

        let encoder = builder.into_inner().expect("expected tar finalize");
        encoder.finish().expect("expected gzip finalize");
    }

    let writes = extract_skill_archive_writes(
        &skill_root,
        &tar_bytes,
        "https://updates.codemod.com/codemod.tar.gz",
    )
    .expect("expected staged writes");

    assert!(writes
        .iter()
        .any(|write| write.path == skill_root.join("SKILL.md")));
    assert!(writes
        .iter()
        .any(|write| { write.path == skill_root.join("references/core/search-and-discovery.md") }));
}

#[test]
fn remote_auto_safe_update_end_to_end_across_supported_harnesses() {
    let _env_lock = ENV_GUARD.lock().expect("expected env lock");
    let runtime = Runtime::new().expect("expected runtime");
    runtime.block_on(async {
        let signing_key = signing_key_fixture();
        let public_key = base64::engine::general_purpose::STANDARD
            .encode(signing_key.verifying_key().to_bytes());

        for harness in [
            Harness::Claude,
            Harness::Goose,
            Harness::Opencode,
            Harness::Cursor,
        ] {
            let temp_dir = tempfile::tempdir().expect("expected temp dir");
            let skill_path = harness_skill_path(temp_dir.path(), harness);
            fs::create_dir_all(skill_path.parent().expect("expected parent for skill path"))
                .expect("expected skill dir");
            fs::write(&skill_path, b"old-skill-content").expect("expected old skill content");

            let harness_name = harness.as_str().to_string();
            let artifact_path = format!("/artifacts/{harness_name}/codemod.md");
            let manifest_path = format!("/manifest/{harness_name}");
            let artifact_bytes =
                format!("---\nname: codemod\nharness: {harness_name}\n---\n").into_bytes();

            let server = TestHttpServer::start_with_builder(|base_url| {
                let manifest = ManagedUpdateManifest {
                    schema_version: "1".to_string(),
                    generated_at: None,
                    components: vec![ManagedUpdateManifestComponent {
                        id: "codemod".to_string(),
                        kind: "skill".to_string(),
                        version: "2.0.0".to_string(),
                        checksum_sha256: sha256_hex(&artifact_bytes),
                        source_url: format!("{base_url}{artifact_path}"),
                        min_cli_version: None,
                        max_cli_version: None,
                        harnesses: Some(vec![harness_name.clone()]),
                    }],
                };
                let manifest_bytes =
                    serde_json::to_vec(&manifest).expect("expected manifest serialization");
                let signature = signing_key.sign(&manifest_bytes);
                let signature_header = format!(
                    "kid=test;sig={}",
                    base64::engine::general_purpose::STANDARD.encode(signature.to_bytes())
                );

                HashMap::from([
                    (
                        manifest_path.clone(),
                        TestHttpFixture {
                            status: StatusCode::OK,
                            body: manifest_bytes,
                            headers: vec![(
                                MANAGED_UPDATE_MANIFEST_SIGNATURES_HEADER.to_string(),
                                signature_header,
                            )],
                        },
                    ),
                    (
                        artifact_path.clone(),
                        TestHttpFixture {
                            status: StatusCode::OK,
                            body: artifact_bytes.clone(),
                            headers: Vec::new(),
                        },
                    ),
                ])
            })
            .await;

            let remote_source = format!("{}{manifest_path}", server.base_url);
            let _env_restore = EnvRestoreGuard::set(&[
                (
                    MANAGED_UPDATE_MANIFEST_PUBLIC_KEYS_ENV_VAR,
                    manifest_public_keys_env_value(&public_key),
                ),
                (
                    "CODEMOD_AGENT_UPDATE_LOCK_PATH",
                    temp_dir.path().join("auto-safe.lock").display().to_string(),
                ),
            ]);

            let context = resolve_update_policy_context(&UpdatePolicyResolveOptions {
                mode: UpdatePolicyMode::AutoSafe,
                remote_source,
                require_signed_manifest: Some(true),
            })
            .await
            .expect("expected update policy context");
            let snapshot = context
                .remote_manifest
                .as_ref()
                .expect("expected remote manifest");
            assert!(snapshot.authenticity_verified);

            let local_components = vec![ManagedComponentSnapshot {
                id: "codemod".to_string(),
                kind: ManagedComponentKind::Skill,
                path: skill_path.clone(),
                version: Some("1.0.0".to_string()),
            }];
            let decisions =
                build_component_reconcile_decisions(&context, harness, &local_components);
            assert_eq!(decisions.len(), 1);
            assert_eq!(
                decisions[0].status,
                ReconcileDecisionStatus::UpdateAvailable
            );

            let apply =
                maybe_apply_auto_safe_updates(&context, &decisions, &local_components).await;
            let result = apply.result.expect("expected auto-safe result");
            assert_eq!(result.applied, 1);
            assert_eq!(result.failed, 0);
            assert!(!result.rolled_back);
            assert_eq!(
                fs::read_to_string(&skill_path).expect("expected updated skill"),
                String::from_utf8(artifact_bytes).expect("expected utf8 artifact"),
            );

            server.shutdown().await;
        }
    });
}

#[test]
fn remote_auto_safe_update_checksum_mismatch_fails_without_writing_partial_content() {
    let _env_lock = ENV_GUARD.lock().expect("expected env lock");
    let runtime = Runtime::new().expect("expected runtime");
    runtime.block_on(async {
        let signing_key = signing_key_fixture();
        let public_key = base64::engine::general_purpose::STANDARD
            .encode(signing_key.verifying_key().to_bytes());

        let temp_dir = tempfile::tempdir().expect("expected temp dir");
        let skill_path = harness_skill_path(temp_dir.path(), Harness::Claude);
        fs::create_dir_all(skill_path.parent().expect("expected parent for skill path"))
            .expect("expected skill dir");
        fs::write(&skill_path, b"old-skill-content").expect("expected old skill content");

        let artifact_path = "/artifacts/codemod.md".to_string();
        let manifest_path = "/manifest/claude".to_string();
        let artifact_bytes = b"new-skill-content".to_vec();

        let server = TestHttpServer::start_with_builder(|base_url| {
            let manifest = ManagedUpdateManifest {
                schema_version: "1".to_string(),
                generated_at: None,
                components: vec![ManagedUpdateManifestComponent {
                    id: "codemod".to_string(),
                    kind: "skill".to_string(),
                    version: "2.0.0".to_string(),
                    checksum_sha256:
                        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                            .to_string(),
                    source_url: format!("{base_url}{artifact_path}"),
                    min_cli_version: None,
                    max_cli_version: None,
                    harnesses: Some(vec!["claude".to_string()]),
                }],
            };
            let manifest_bytes = serde_json::to_vec(&manifest).expect("expected serialization");
            let signature = signing_key.sign(&manifest_bytes);
            let signature_header = format!(
                "kid=test;sig={}",
                base64::engine::general_purpose::STANDARD.encode(signature.to_bytes())
            );

            HashMap::from([
                (
                    manifest_path.clone(),
                    TestHttpFixture {
                        status: StatusCode::OK,
                        body: manifest_bytes,
                        headers: vec![(
                            MANAGED_UPDATE_MANIFEST_SIGNATURES_HEADER.to_string(),
                            signature_header,
                        )],
                    },
                ),
                (
                    artifact_path.clone(),
                    TestHttpFixture {
                        status: StatusCode::OK,
                        body: artifact_bytes.clone(),
                        headers: Vec::new(),
                    },
                ),
            ])
        })
        .await;

        let _env_restore = EnvRestoreGuard::set(&[
            (
                MANAGED_UPDATE_MANIFEST_PUBLIC_KEYS_ENV_VAR,
                manifest_public_keys_env_value(&public_key),
            ),
            (
                "CODEMOD_AGENT_UPDATE_LOCK_PATH",
                temp_dir.path().join("auto-safe.lock").display().to_string(),
            ),
        ]);

        let context = resolve_update_policy_context(&UpdatePolicyResolveOptions {
            mode: UpdatePolicyMode::AutoSafe,
            remote_source: format!("{}{manifest_path}", server.base_url),
            require_signed_manifest: Some(true),
        })
        .await
        .expect("expected update policy context");
        let local_components = vec![ManagedComponentSnapshot {
            id: "codemod".to_string(),
            kind: ManagedComponentKind::Skill,
            path: skill_path.clone(),
            version: Some("1.0.0".to_string()),
        }];
        let decisions =
            build_component_reconcile_decisions(&context, Harness::Claude, &local_components);
        assert_eq!(
            decisions[0].status,
            ReconcileDecisionStatus::UpdateAvailable
        );

        let apply = maybe_apply_auto_safe_updates(&context, &decisions, &local_components).await;
        let result = apply.result.expect("expected result");
        assert_eq!(result.applied, 0);
        assert!(result.failed >= 1);
        assert_eq!(
            fs::read_to_string(&skill_path).expect("expected unchanged skill content"),
            "old-skill-content"
        );

        server.shutdown().await;
    });
}

#[test]
fn remote_auto_safe_update_skips_incompatible_components() {
    let _env_lock = ENV_GUARD.lock().expect("expected env lock");
    let runtime = Runtime::new().expect("expected runtime");
    runtime.block_on(async {
        let signing_key = signing_key_fixture();
        let public_key = base64::engine::general_purpose::STANDARD
            .encode(signing_key.verifying_key().to_bytes());

        let temp_dir = tempfile::tempdir().expect("expected temp dir");
        let skill_path = harness_skill_path(temp_dir.path(), Harness::Claude);
        fs::create_dir_all(skill_path.parent().expect("expected parent for skill path"))
            .expect("expected skill dir");
        fs::write(&skill_path, b"old-skill-content").expect("expected old skill content");

        let artifact_path = "/artifacts/codemod.md".to_string();
        let manifest_path = "/manifest/claude".to_string();
        let artifact_bytes = b"new-skill-content".to_vec();

        let server = TestHttpServer::start_with_builder(|base_url| {
            let manifest = ManagedUpdateManifest {
                schema_version: "1".to_string(),
                generated_at: None,
                components: vec![ManagedUpdateManifestComponent {
                    id: "codemod".to_string(),
                    kind: "skill".to_string(),
                    version: "2.0.0".to_string(),
                    checksum_sha256: sha256_hex(&artifact_bytes),
                    source_url: format!("{base_url}{artifact_path}"),
                    min_cli_version: Some("999.0.0".to_string()),
                    max_cli_version: None,
                    harnesses: Some(vec!["claude".to_string()]),
                }],
            };
            let manifest_bytes = serde_json::to_vec(&manifest).expect("expected serialization");
            let signature = signing_key.sign(&manifest_bytes);
            let signature_header = format!(
                "kid=test;sig={}",
                base64::engine::general_purpose::STANDARD.encode(signature.to_bytes())
            );

            HashMap::from([
                (
                    manifest_path.clone(),
                    TestHttpFixture {
                        status: StatusCode::OK,
                        body: manifest_bytes,
                        headers: vec![(
                            MANAGED_UPDATE_MANIFEST_SIGNATURES_HEADER.to_string(),
                            signature_header,
                        )],
                    },
                ),
                (
                    artifact_path.clone(),
                    TestHttpFixture {
                        status: StatusCode::OK,
                        body: artifact_bytes.clone(),
                        headers: Vec::new(),
                    },
                ),
            ])
        })
        .await;

        let _env_restore = EnvRestoreGuard::set(&[
            (
                MANAGED_UPDATE_MANIFEST_PUBLIC_KEYS_ENV_VAR,
                manifest_public_keys_env_value(&public_key),
            ),
            (
                "CODEMOD_AGENT_UPDATE_LOCK_PATH",
                temp_dir.path().join("auto-safe.lock").display().to_string(),
            ),
        ]);

        let context = resolve_update_policy_context(&UpdatePolicyResolveOptions {
            mode: UpdatePolicyMode::AutoSafe,
            remote_source: format!("{}{manifest_path}", server.base_url),
            require_signed_manifest: Some(true),
        })
        .await
        .expect("expected update policy context");
        let local_components = vec![ManagedComponentSnapshot {
            id: "codemod".to_string(),
            kind: ManagedComponentKind::Skill,
            path: skill_path.clone(),
            version: Some("1.0.0".to_string()),
        }];
        let decisions =
            build_component_reconcile_decisions(&context, Harness::Claude, &local_components);
        assert_eq!(decisions[0].status, ReconcileDecisionStatus::Incompatible);

        let apply = maybe_apply_auto_safe_updates(&context, &decisions, &local_components).await;
        let result = apply.result.expect("expected result");
        assert_eq!(result.attempted, 0);
        assert_eq!(result.applied, 0);
        assert_eq!(
            fs::read_to_string(&skill_path).expect("expected unchanged skill content"),
            "old-skill-content"
        );

        server.shutdown().await;
    });
}

#[test]
fn remote_auto_safe_update_lock_contention_is_reported_deterministically() {
    let _env_lock = ENV_GUARD.lock().expect("expected env lock");
    let runtime = Runtime::new().expect("expected runtime");
    runtime.block_on(async {
        let signing_key = signing_key_fixture();
        let public_key = base64::engine::general_purpose::STANDARD
            .encode(signing_key.verifying_key().to_bytes());

        let temp_dir = tempfile::tempdir().expect("expected temp dir");
        let skill_path = harness_skill_path(temp_dir.path(), Harness::Claude);
        fs::create_dir_all(skill_path.parent().expect("expected parent for skill path"))
            .expect("expected skill dir");
        fs::write(&skill_path, b"old-skill-content").expect("expected old skill content");

        let artifact_path = "/artifacts/codemod.md".to_string();
        let manifest_path = "/manifest/claude".to_string();
        let artifact_bytes = b"new-skill-content".to_vec();

        let server = TestHttpServer::start_with_builder(|base_url| {
            let manifest = ManagedUpdateManifest {
                schema_version: "1".to_string(),
                generated_at: None,
                components: vec![ManagedUpdateManifestComponent {
                    id: "codemod".to_string(),
                    kind: "skill".to_string(),
                    version: "2.0.0".to_string(),
                    checksum_sha256: sha256_hex(&artifact_bytes),
                    source_url: format!("{base_url}{artifact_path}"),
                    min_cli_version: None,
                    max_cli_version: None,
                    harnesses: Some(vec!["claude".to_string()]),
                }],
            };
            let manifest_bytes = serde_json::to_vec(&manifest).expect("expected serialization");
            let signature = signing_key.sign(&manifest_bytes);
            let signature_header = format!(
                "kid=test;sig={}",
                base64::engine::general_purpose::STANDARD.encode(signature.to_bytes())
            );

            HashMap::from([
                (
                    manifest_path.clone(),
                    TestHttpFixture {
                        status: StatusCode::OK,
                        body: manifest_bytes,
                        headers: vec![(
                            MANAGED_UPDATE_MANIFEST_SIGNATURES_HEADER.to_string(),
                            signature_header,
                        )],
                    },
                ),
                (
                    artifact_path.clone(),
                    TestHttpFixture {
                        status: StatusCode::OK,
                        body: artifact_bytes.clone(),
                        headers: Vec::new(),
                    },
                ),
            ])
        })
        .await;

        let lock_path = temp_dir.path().join("contended-auto-safe.lock");
        fs::write(
            &lock_path,
            serde_json::to_vec(&serde_json::json!({
                "pid": 4242,
                "acquired_at_epoch_secs": std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            }))
            .expect("expected lock serialization"),
        )
        .expect("expected lock write");

        let _env_restore = EnvRestoreGuard::set(&[
            (
                MANAGED_UPDATE_MANIFEST_PUBLIC_KEYS_ENV_VAR,
                manifest_public_keys_env_value(&public_key),
            ),
            (
                "CODEMOD_AGENT_UPDATE_LOCK_PATH",
                lock_path.display().to_string(),
            ),
            ("CODEMOD_AGENT_UPDATE_LOCK_TIMEOUT_MS", "40".to_string()),
            ("CODEMOD_AGENT_UPDATE_LOCK_RETRY_MS", "10".to_string()),
            ("CODEMOD_AGENT_UPDATE_LOCK_STALE_SECS", "600".to_string()),
        ]);

        let context = resolve_update_policy_context(&UpdatePolicyResolveOptions {
            mode: UpdatePolicyMode::AutoSafe,
            remote_source: format!("{}{manifest_path}", server.base_url),
            require_signed_manifest: Some(true),
        })
        .await
        .expect("expected update policy context");
        let local_components = vec![ManagedComponentSnapshot {
            id: "codemod".to_string(),
            kind: ManagedComponentKind::Skill,
            path: skill_path.clone(),
            version: Some("1.0.0".to_string()),
        }];
        let decisions =
            build_component_reconcile_decisions(&context, Harness::Claude, &local_components);
        assert_eq!(
            decisions[0].status,
            ReconcileDecisionStatus::UpdateAvailable
        );

        let apply = maybe_apply_auto_safe_updates(&context, &decisions, &local_components).await;
        let result = apply.result.expect("expected result");
        assert_eq!(result.applied, 0);
        assert_eq!(result.skipped, 1);
        assert_eq!(
            result.rollback_reason.as_deref(),
            Some("lock_acquire_failed")
        );
        assert!(apply
            .warnings
            .iter()
            .any(|warning| warning.contains("timed out after 40ms")));
        assert_eq!(
            fs::read_to_string(&skill_path).expect("expected unchanged skill content"),
            "old-skill-content"
        );

        server.shutdown().await;
    });
}

#[test]
fn resolve_update_policy_context_handles_rate_limited_manifest_endpoint_gracefully() {
    let _env_lock = ENV_GUARD.lock().expect("expected env lock");
    let runtime = Runtime::new().expect("expected runtime");
    runtime.block_on(async {
        let manifest_path = "/manifest/rate-limited".to_string();
        let server = TestHttpServer::start_with_builder(|_| {
            HashMap::from([(
                manifest_path.clone(),
                TestHttpFixture {
                    status: StatusCode::TOO_MANY_REQUESTS,
                    body: b"rate limited".to_vec(),
                    headers: vec![
                        ("Retry-After".to_string(), "60".to_string()),
                        (
                            "X-RateLimit-Reset".to_string(),
                            (std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs()
                                + 60)
                                .to_string(),
                        ),
                    ],
                },
            )])
        })
        .await;

        let context = resolve_update_policy_context(&UpdatePolicyResolveOptions {
            mode: UpdatePolicyMode::Notify,
            remote_source: format!("{}{manifest_path}", server.base_url),
            require_signed_manifest: Some(true),
        })
        .await
        .expect("expected update policy context");

        assert!(context.fallback_applied);
        assert!(context.remote_manifest.is_none());
        assert!(context.warnings.iter().any(|warning| {
            warning.contains("HTTP 429") && warning.contains("retry after 60s")
        }));

        server.shutdown().await;
    });
}

#[test]
fn resolve_update_policy_context_truncates_large_error_body_in_warnings() {
    let _env_lock = ENV_GUARD.lock().expect("expected env lock");
    let runtime = Runtime::new().expect("expected runtime");
    runtime.block_on(async {
        let manifest_path = "/manifest/rate-limited-large".to_string();
        let oversized_body = "x".repeat(2_048);
        let server = TestHttpServer::start_with_builder(|_| {
            HashMap::from([(
                manifest_path.clone(),
                TestHttpFixture {
                    status: StatusCode::TOO_MANY_REQUESTS,
                    body: oversized_body.clone().into_bytes(),
                    headers: vec![("Retry-After".to_string(), "60".to_string())],
                },
            )])
        })
        .await;

        let context = resolve_update_policy_context(&UpdatePolicyResolveOptions {
            mode: UpdatePolicyMode::Notify,
            remote_source: format!("{}{manifest_path}", server.base_url),
            require_signed_manifest: Some(true),
        })
        .await
        .expect("expected update policy context");

        let warning = context
            .warnings
            .iter()
            .find(|warning| warning.contains("HTTP 429"))
            .expect("expected HTTP warning");

        assert!(warning.contains("retry after 60s"));
        assert!(warning.contains("..."));
        assert!(!warning.contains(&oversized_body));

        server.shutdown().await;
    });
}

#[test]
fn resolve_update_policy_context_hides_server_configuration_details_for_5xx() {
    let _env_lock = ENV_GUARD.lock().expect("expected env lock");
    let runtime = Runtime::new().expect("expected runtime");
    runtime.block_on(async {
        let manifest_path = "/manifest/unavailable".to_string();
        let server = TestHttpServer::start_with_builder(|_| {
            HashMap::from([(
                manifest_path.clone(),
                TestHttpFixture {
                    status: StatusCode::SERVICE_UNAVAILABLE,
                    body: br#"{"error":"Service Unavailable","message":"Managed update manifest is not configured. Set MANAGED_UPDATE_MANIFEST_JSON."}"#.to_vec(),
                    headers: Vec::new(),
                },
            )])
        })
        .await;

        let context = resolve_update_policy_context(&UpdatePolicyResolveOptions {
            mode: UpdatePolicyMode::Notify,
            remote_source: format!("{}{manifest_path}", server.base_url),
            require_signed_manifest: Some(true),
        })
        .await
        .expect("expected update policy context");

        let warning = context
            .warnings
            .iter()
            .find(|warning| warning.contains("HTTP 503"))
            .expect("expected HTTP 503 warning");

        assert!(warning.contains("remote manifest service unavailable"));
        assert!(!warning.contains("MANAGED_UPDATE_MANIFEST_JSON"));

        server.shutdown().await;
    });
}

#[test]
fn resolve_update_policy_context_suppresses_expected_registry_404_manifest_warning() {
    let _env_lock = ENV_GUARD.lock().expect("expected env lock");
    let runtime = Runtime::new().expect("expected runtime");
    runtime.block_on(async {
        let manifest_path = "/api/v1/ai/managed-components/manifest".to_string();
        let server = TestHttpServer::start_with_builder(|_| {
            HashMap::from([(
                manifest_path.clone(),
                TestHttpFixture {
                    status: StatusCode::NOT_FOUND,
                    body: b"not found".to_vec(),
                    headers: Vec::new(),
                },
            )])
        })
        .await;

        let _env_restore =
            EnvRestoreGuard::set(&[("CODEMOD_REGISTRY_URL", server.base_url.clone())]);
        let context = resolve_update_policy_context(&UpdatePolicyResolveOptions {
            mode: UpdatePolicyMode::AutoSafe,
            remote_source: "registry".to_string(),
            require_signed_manifest: Some(true),
        })
        .await
        .expect("expected update policy context");

        assert!(context.fallback_applied);
        assert!(context.remote_manifest.is_none());
        assert!(context.warnings.is_empty());

        server.shutdown().await;
    });
}

#[test]
fn maybe_apply_auto_safe_updates_silently_skips_when_remote_manifest_is_unavailable() {
    let runtime = Runtime::new().expect("expected runtime");
    runtime.block_on(async {
        let context = UpdatePolicyContext {
            mode: UpdatePolicyMode::AutoSafe,
            remote_source: MANAGED_UPDATE_POLICY_LOCAL_SOURCE.to_string(),
            fallback_applied: true,
            remote_manifest: None,
            warnings: Vec::new(),
        };

        let apply = maybe_apply_auto_safe_updates(&context, &[], &[]).await;
        assert!(apply.warnings.is_empty());
        assert!(apply.result.is_none());
    });
}
