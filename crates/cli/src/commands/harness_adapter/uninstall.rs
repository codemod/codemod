//! `codemod ai uninstall`: removes what `codemod ai` installed for one agent
//! and scope, and leaves anything that doesn't look like Codemod's own.

use super::doctor::installed_setups;
use super::*;
use std::collections::BTreeMap;

const PERIODIC_RUNNER_SUFFIX: &str = "codemod/periodic-update/check-updates.sh";

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RemovedItem {
    pub what: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct KeptItem {
    pub what: String,
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PendingChange {
    Write(String),
    DeleteFile,
    DeleteDir,
}

#[derive(Debug)]
pub struct UninstallPlan {
    pub harness: Harness,
    pub scope: InstallScope,
    pub removed: Vec<RemovedItem>,
    pub kept: Vec<KeptItem>,
    changes: BTreeMap<PathBuf, PendingChange>,
    scope_root: PathBuf,
}

impl UninstallPlan {
    pub fn is_empty(&self) -> bool {
        self.removed.is_empty()
    }

    /// Reads a file as it will be once earlier steps of the plan are applied.
    fn read(&self, path: &Path) -> Option<String> {
        match self.changes.get(path) {
            Some(PendingChange::Write(content)) => Some(content.clone()),
            Some(_) => None,
            None => fs::read_to_string(path).ok(),
        }
    }

    fn remove_file(&mut self, what: &str, path: PathBuf) {
        self.record(what, &path);
        self.changes.insert(path, PendingChange::DeleteFile);
    }

    fn remove_dir(&mut self, what: &str, path: PathBuf) {
        self.record(what, &path);
        self.changes.insert(path, PendingChange::DeleteDir);
    }

    /// Writes the edited file, or deletes it when the edit leaves nothing of
    /// the user's behind.
    fn edit_file(&mut self, what: &str, path: PathBuf, content: Option<String>) {
        self.record(what, &path);
        let change = content.map_or(PendingChange::DeleteFile, PendingChange::Write);
        self.changes.insert(path, change);
    }

    fn record(&mut self, what: &str, path: &Path) {
        if !self
            .removed
            .iter()
            .any(|item| item.what == what && item.path == path)
        {
            self.removed.push(RemovedItem {
                what: what.to_string(),
                path: path.to_path_buf(),
            });
        }
    }

    fn keep(&mut self, what: &str, path: &Path, reason: impl Into<String>) {
        self.kept.push(KeptItem {
            what: what.to_string(),
            path: path.to_path_buf(),
            reason: reason.into(),
        });
    }

    pub fn apply(&self) -> AdapterResult<()> {
        let failed = |path: &Path, error: std::io::Error| {
            HarnessAdapterError::InstallFailed(format!(
                "Failed to remove Codemod from {}: {error}",
                path.display()
            ))
        };
        for (path, change) in &self.changes {
            match change {
                PendingChange::Write(content) => {
                    fs::write(path, content).map_err(|error| failed(path, error))?
                }
                PendingChange::DeleteFile if path.exists() => {
                    fs::remove_file(path).map_err(|error| failed(path, error))?
                }
                PendingChange::DeleteDir if path.exists() => {
                    fs::remove_dir_all(path).map_err(|error| failed(path, error))?
                }
                _ => {}
            }
            if !matches!(change, PendingChange::Write(_)) {
                remove_empty_parents(path, &self.scope_root);
            }
        }
        Ok(())
    }
}

/// Folders such as `.cursor/commands` exist only because install created
/// them; drop them once empty, never going above the project or home folder.
fn remove_empty_parents(path: &Path, scope_root: &Path) {
    for dir in path.ancestors().skip(1) {
        if !dir.starts_with(scope_root) || dir == scope_root || fs::remove_dir(dir).is_err() {
            return;
        }
    }
}

pub fn plan_uninstall(harness: Harness, scope: InstallScope) -> AdapterResult<UninstallPlan> {
    plan_uninstall_with_runtime(harness, scope, &RuntimePaths::current()?)
}

/// The agents with Codemod set up at `scope`, for `--harness auto`.
pub fn installed_harnesses(scope: InstallScope) -> AdapterResult<Vec<Harness>> {
    let runtime_paths = RuntimePaths::current()?;
    Ok(installed_setups(&runtime_paths)
        .into_iter()
        .filter(|(_, setup_scope)| *setup_scope == scope)
        .map(|(harness, _)| harness)
        .collect())
}

pub(crate) fn plan_uninstall_with_runtime(
    harness: Harness,
    scope: InstallScope,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<UninstallPlan> {
    let scope_root = match scope {
        InstallScope::Project => runtime_paths.cwd.clone(),
        InstallScope::User => runtime_paths.home_dir.clone().ok_or_else(|| {
            HarnessAdapterError::InstallFailed(
                "Could not determine home directory for --user uninstall".to_string(),
            )
        })?,
    };
    let mut plan = UninstallPlan {
        harness,
        scope,
        removed: Vec::new(),
        kept: Vec::new(),
        changes: BTreeMap::new(),
        scope_root,
    };
    let other_setups = installed_setups(runtime_paths)
        .into_iter()
        .filter(|&(other, other_scope)| other_scope == scope && other != harness)
        .collect::<Vec<_>>();

    plan_skill(&mut plan, &other_setups, runtime_paths)?;
    plan_command(&mut plan, runtime_paths)?;
    if harness_supports_mcp(harness) {
        let config_path = mcp_config_path_for_harness(harness, scope, runtime_paths)?;
        plan_mcp_entry(&mut plan, config_path, |_| true);
    }
    if (harness, scope) == (Harness::Claude, InstallScope::User) {
        // Earlier releases wrote the Claude user server to `~/.mcp.json`.
        if let Some(home) = &runtime_paths.home_dir {
            plan_mcp_entry(&mut plan, home.join(".mcp.json"), launches_codemod);
        }
    }
    plan_discovery_hints(&mut plan, &other_setups, runtime_paths)?;
    plan_periodic_update(&mut plan, runtime_paths)?;

    let codemod_root = harness_root_for_scope(harness, scope, runtime_paths)?.join("codemod");
    if codemod_root.is_dir() {
        plan.remove_dir("Codemod install record and update runner", codemod_root);
    }
    Ok(plan)
}

fn shared_with(
    other_setups: &[(Harness, InstallScope)],
    uses: impl Fn(Harness, InstallScope) -> bool,
) -> Option<Harness> {
    other_setups
        .iter()
        .find(|&&(harness, scope)| uses(harness, scope))
        .map(|&(harness, _)| harness)
}

fn plan_skill(
    plan: &mut UninstallPlan,
    other_setups: &[(Harness, InstallScope)],
    runtime_paths: &RuntimePaths,
) -> AdapterResult<()> {
    const WHAT: &str = "Codemod skill";
    let skill_dir =
        skills_root_for_harness(plan.harness, plan.scope, runtime_paths)?.join(MCS_SKILL_DIR_NAME);
    let Some(content) = plan.read(&skill_dir.join(SKILL_FILE_NAME)) else {
        return Ok(());
    };
    if !content.contains(MCS_COMPATIBILITY_MARKER) {
        plan.keep(WHAT, &skill_dir, "it isn't the skill Codemod installs");
        return Ok(());
    }
    let shared = shared_with(other_setups, |harness, scope| {
        skills_root_for_harness(harness, scope, runtime_paths)
            .is_ok_and(|root| root.join(MCS_SKILL_DIR_NAME) == skill_dir)
    });
    match shared {
        Some(other) => plan.keep(
            WHAT,
            &skill_dir,
            format!("{} uses it too", other.display_name()),
        ),
        None => plan.remove_dir(WHAT, skill_dir),
    }
    Ok(())
}

fn is_codemod_command_file(content: &str) -> bool {
    content == MCS_COMMAND_MD || content.contains("installed `codemod` skill")
}

fn plan_command(plan: &mut UninstallPlan, runtime_paths: &RuntimePaths) -> AdapterResult<()> {
    const WHAT: &str = "/codemod command";
    if plan.harness == Harness::Goose {
        if let Some((recipe_path, config_path)) =
            goose_mcs_command_paths(plan.scope, runtime_paths)?
        {
            plan_goose_command(plan, recipe_path, config_path);
        }
        return Ok(());
    }
    let Some(path) =
        mcs_command_entrypoint_path_for_harness(plan.harness, plan.scope, runtime_paths)?
    else {
        return Ok(());
    };
    match plan.read(&path) {
        Some(content) if is_codemod_command_file(&content) => plan.remove_file(WHAT, path),
        Some(_) => plan.keep(WHAT, &path, "it was edited"),
        None => {}
    }
    Ok(())
}

fn plan_goose_command(plan: &mut UninstallPlan, recipe_path: PathBuf, config_path: PathBuf) {
    const WHAT: &str = "/codemod command";
    match plan.read(&recipe_path) {
        Some(content) if content == goose_mcs_recipe_content() => {
            plan.remove_file(WHAT, recipe_path.clone())
        }
        Some(_) => plan.keep(WHAT, &recipe_path, "it was edited"),
        None => {}
    }
    let Some(mut config) = plan
        .read(&config_path)
        .and_then(|content| serde_yaml::from_str::<serde_yaml::Value>(&content).ok())
    else {
        return;
    };
    let recipe = recipe_path.to_string_lossy().to_string();
    let Some(root) = config.as_mapping_mut() else {
        return;
    };
    let key = serde_yaml::Value::String("slash_commands".to_string());
    let Some(commands) = root
        .get_mut(&key)
        .and_then(serde_yaml::Value::as_sequence_mut)
    else {
        return;
    };
    let before = commands.len();
    commands.retain(|command| {
        let field = |name: &str| command.get(name).and_then(serde_yaml::Value::as_str);
        !(field("command") == Some(MCS_COMMAND_NAME) && field("recipe_path") == Some(&recipe))
    });
    if commands.len() == before {
        return;
    }
    if commands.is_empty() {
        root.remove(&key);
    }
    if root.is_empty() {
        plan.edit_file(WHAT, config_path, None);
    } else if let Ok(serialized) = serde_yaml::to_string(&config) {
        plan.edit_file(WHAT, config_path, Some(serialized));
    }
}

/// An MCP entry whose launcher runs the Codemod CLI.
fn launches_codemod(entry: &Value) -> bool {
    let mentions_codemod = |text: &str| {
        text == CODEMOD_CLI_COMMAND
            || text.starts_with("codemod@")
            || Path::new(text).file_stem() == Some(CODEMOD_CLI_COMMAND.as_ref())
    };
    entry
        .get("command")
        .and_then(Value::as_str)
        .into_iter()
        .chain(
            entry
                .get("args")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str),
        )
        .any(mentions_codemod)
}

fn plan_mcp_entry(
    plan: &mut UninstallPlan,
    config_path: PathBuf,
    should_remove: impl Fn(&Value) -> bool,
) {
    const WHAT: &str = "Codemod MCP server";
    let Some(content) = plan.read(&config_path) else {
        return;
    };
    let edited = match plan.harness {
        Harness::Codex => remove_toml_mcp_entry(&content),
        Harness::Opencode => remove_opencode_mcp_entry(&content),
        _ => {
            let is_claude_state = config_path.file_name() == Some(".claude.json".as_ref());
            remove_json_mcp_entry(&content, !is_claude_state, &should_remove)
        }
    };
    match edited {
        Ok(Some(edited)) => plan.edit_file(WHAT, config_path, edited),
        Ok(None) => {}
        Err(problem) => plan.keep(
            WHAT,
            &config_path,
            format!("the file {problem}; remove the `codemod` server by hand"),
        ),
    }
}

/// `Ok(None)`: nothing to remove. `Ok(Some(None))`: delete the file.
type FileEdit = Result<Option<Option<String>>, String>;

fn remove_json_mcp_entry(
    content: &str,
    delete_when_empty: bool,
    should_remove: impl Fn(&Value) -> bool,
) -> FileEdit {
    let mut config: Value =
        serde_json::from_str(content).map_err(|_| "isn't valid JSON".to_string())?;
    let Some(root) = config.as_object_mut() else {
        return Ok(None);
    };
    let Some(servers) = root.get_mut("mcpServers").and_then(Value::as_object_mut) else {
        return Ok(None);
    };
    if !servers.get(MCP_SERVER_NAME).is_some_and(&should_remove) {
        return Ok(None);
    }
    servers.remove(MCP_SERVER_NAME);
    if servers.is_empty() {
        root.remove("mcpServers");
    }
    if root.is_empty() && delete_when_empty {
        return Ok(Some(None));
    }
    Ok(Some(Some(pretty_json(&config))))
}

fn pretty_json(value: &Value) -> String {
    format!(
        "{}\n",
        serde_json::to_string_pretty(value).unwrap_or_default()
    )
}

fn remove_opencode_mcp_entry(content: &str) -> FileEdit {
    let invalid = |_| "isn't valid JSON".to_string();
    let root =
        CstRootNode::parse(content, &jsonc_parser::ParseOptions::default()).map_err(invalid)?;
    let Some(root_object) = root.object_value() else {
        return Ok(None);
    };
    let Some(mcp) = root_object.object_value("mcp") else {
        return Ok(None);
    };
    let Some(entry) = mcp.get(MCP_SERVER_NAME) else {
        return Ok(None);
    };
    entry.remove();
    if mcp.properties().is_empty() {
        if let Some(mcp_prop) = root_object.get("mcp") {
            mcp_prop.remove();
        }
    }
    let leftover = root_object
        .properties()
        .iter()
        .filter_map(|prop| prop.name()?.decoded_value().ok())
        .any(|name| name != "$schema");
    if !leftover {
        return Ok(Some(None));
    }
    let mut edited = root.to_string();
    if !edited.ends_with('\n') {
        edited.push('\n');
    }
    Ok(Some(Some(edited)))
}

fn remove_toml_mcp_entry(content: &str) -> FileEdit {
    let mut document = content
        .parse::<DocumentMut>()
        .map_err(|_| "isn't valid TOML".to_string())?;
    let Some(servers) = document
        .get_mut("mcp_servers")
        .and_then(Item::as_table_like_mut)
    else {
        return Ok(None);
    };
    if servers.remove(MCP_SERVER_NAME).is_none() {
        return Ok(None);
    }
    if servers.is_empty() {
        document.remove("mcp_servers");
    }
    Ok(Some(toml_or_delete(&document)))
}

fn toml_or_delete(document: &DocumentMut) -> Option<String> {
    (!document.as_table().is_empty()).then(|| toml_document_with_trailing_newline(document))
}

/// Removes a managed `begin`..`end` block and the blank lines around it.
/// `None` when the block is absent; `Some(None)` when nothing else is left.
fn remove_managed_block(existing: &str, begin: &str, end: &str) -> Option<Option<String>> {
    let start = existing.find(begin)?;
    let stop = existing[start..].find(end)? + start + end.len();
    let before = existing[..start].trim_end();
    let after = existing[stop..].trim_start();
    let rest = match (before.is_empty(), after.is_empty()) {
        (true, true) => return Some(None),
        (true, false) => after.to_string(),
        (false, true) => format!("{before}\n"),
        (false, false) => format!("{before}\n\n{after}"),
    };
    Some(Some(rest))
}

fn plan_discovery_hints(
    plan: &mut UninstallPlan,
    other_setups: &[(Harness, InstallScope)],
    runtime_paths: &RuntimePaths,
) -> AdapterResult<()> {
    const WHAT: &str = "Codemod section";
    for guide in discovery_guide_paths_with_runtime(plan.harness, plan.scope, runtime_paths)? {
        let Some(content) = plan.read(&guide) else {
            continue;
        };
        let Some(edited) = remove_managed_block(
            &content,
            SKILL_DISCOVERY_SECTION_BEGIN,
            SKILL_DISCOVERY_SECTION_END,
        ) else {
            continue;
        };
        let shared = shared_with(other_setups, |harness, scope| {
            discovery_guide_paths_with_runtime(harness, scope, runtime_paths)
                .is_ok_and(|paths| paths.contains(&guide))
        });
        match shared {
            Some(other) => plan.keep(
                WHAT,
                &guide,
                format!("{} uses it too", other.display_name()),
            ),
            None => plan.edit_file(WHAT, guide, edited),
        }
    }
    Ok(())
}

fn plan_periodic_update(
    plan: &mut UninstallPlan,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<()> {
    const WHAT: &str = "Periodic update check";
    let harness_root = harness_root_for_scope(plan.harness, plan.scope, runtime_paths)?;
    let path =
        periodic_update_trigger_strategy(plan.harness, plan.scope, runtime_paths, &harness_root)?
            .integration_path;
    let Some(content) = plan.read(&path) else {
        return Ok(());
    };
    let edited = match plan.harness {
        Harness::Claude => remove_json_runner_hooks(&content, "SessionStart", true),
        Harness::Cursor => remove_json_runner_hooks(
            &content,
            CODEMOD_PERIODIC_TRIGGER_CURSOR_HOOK_EVENT_NAME,
            false,
        ),
        Harness::Opencode => Ok(content.contains("CodemodPeriodicUpdate").then_some(None)),
        Harness::Codex => remove_codex_runner_notify(&content),
        Harness::Goose => Ok(remove_managed_block(
            &content,
            CODEMOD_PERIODIC_TRIGGER_GOOSE_HINTS_BEGIN,
            CODEMOD_PERIODIC_TRIGGER_GOOSE_HINTS_END,
        )),
        Harness::Antigravity => Ok(remove_managed_block(
            &content,
            CODEMOD_PERIODIC_TRIGGER_ANTIGRAVITY_HINTS_BEGIN,
            CODEMOD_PERIODIC_TRIGGER_ANTIGRAVITY_HINTS_END,
        )),
        Harness::Auto => Ok(None),
    };
    match edited {
        Ok(Some(edited)) => plan.edit_file(WHAT, path, edited),
        Ok(None) => {}
        Err(problem) => plan.keep(
            WHAT,
            &path,
            format!("the file {problem}; remove the Codemod update hook by hand"),
        ),
    }
    Ok(())
}

fn runs_codemod_runner(hook: &Value) -> bool {
    hook.get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| {
            command
                .trim_end_matches('"')
                .replace('\\', "/")
                .ends_with(PERIODIC_RUNNER_SUFFIX)
        })
}

/// Claude nests command hooks under `{ hooks: [...] }` entries; Cursor lists
/// `{ command }` entries directly under the event.
fn remove_json_runner_hooks(content: &str, event: &str, nested: bool) -> FileEdit {
    let mut settings: Value =
        serde_json::from_str(content).map_err(|_| "isn't valid JSON".to_string())?;
    let Some(root) = settings.as_object_mut() else {
        return Ok(None);
    };
    let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
        return Ok(None);
    };
    let Some(entries) = hooks.get_mut(event).and_then(Value::as_array_mut) else {
        return Ok(None);
    };
    let before = serde_json::to_string(entries).unwrap_or_default();
    if nested {
        entries.retain_mut(|entry| {
            let Some(entry_hooks) = entry.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            entry_hooks.retain(|hook| !runs_codemod_runner(hook));
            !entry_hooks.is_empty()
        });
    } else {
        entries.retain(|entry| !runs_codemod_runner(entry));
    }
    if serde_json::to_string(entries).unwrap_or_default() == before {
        return Ok(None);
    }
    if entries.is_empty() {
        hooks.remove(event);
    }
    if hooks.is_empty() {
        root.remove("hooks");
    }
    let only_cursor_version = !nested && root.len() == 1 && root.contains_key("version");
    if root.is_empty() || only_cursor_version {
        return Ok(Some(None));
    }
    Ok(Some(Some(pretty_json(&settings))))
}

fn remove_codex_runner_notify(content: &str) -> FileEdit {
    let mut document = content
        .parse::<DocumentMut>()
        .map_err(|_| "isn't valid TOML".to_string())?;
    match read_notify_command(&document) {
        Ok(Some(notify)) if is_codemod_periodic_notify_command(&notify) => {
            document.remove("notify");
            Ok(Some(toml_or_delete(&document)))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{full_install, runtime_paths_with_temp_roots};
    use super::*;

    const ALL_HARNESSES: [Harness; 6] = [
        Harness::Claude,
        Harness::Cursor,
        Harness::Codex,
        Harness::Opencode,
        Harness::Goose,
        Harness::Antigravity,
    ];

    /// Every file under `root` with its content, so two snapshots compare whole trees.
    fn snapshot(root: &Path) -> BTreeMap<PathBuf, String> {
        let mut files = BTreeMap::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(dir) = pending.pop() {
            for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    files.insert(path.strip_prefix(root).unwrap().join(""), String::new());
                    pending.push(path);
                } else {
                    let content = fs::read_to_string(&path).unwrap_or_default();
                    files.insert(path.strip_prefix(root).unwrap().to_path_buf(), content);
                }
            }
        }
        files
    }

    fn uninstall(
        harness: Harness,
        scope: InstallScope,
        runtime_paths: &RuntimePaths,
    ) -> UninstallPlan {
        let plan = plan_uninstall_with_runtime(harness, scope, runtime_paths).unwrap();
        plan.apply().unwrap();
        plan
    }

    fn kept_paths(plan: &UninstallPlan) -> Vec<PathBuf> {
        plan.kept.iter().map(|item| item.path.clone()).collect()
    }

    #[test]
    fn uninstall_leaves_the_project_and_home_folder_as_they_were_before_setup() {
        let mut left_behind = Vec::new();
        for harness in ALL_HARNESSES {
            for scope in [InstallScope::Project, InstallScope::User] {
                let (runtime_paths, temp) = runtime_paths_with_temp_roots();
                // Claude Code keeps its own state here, so the file always exists.
                fs::write(
                    temp.path().join("home/.claude.json"),
                    "{\n  \"numStartups\": 3\n}\n",
                )
                .unwrap();
                let before = snapshot(temp.path());
                full_install(harness, scope, &runtime_paths);
                uninstall(harness, scope, &runtime_paths);
                let after = snapshot(temp.path());
                if after != before {
                    let extra = after
                        .keys()
                        .filter(|path| before.get(*path) != after.get(*path))
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>();
                    left_behind.push(format!("{harness:?} {scope:?}: {}", extra.join(", ")));
                }
            }
        }
        assert_eq!(left_behind, Vec::<String>::new());
    }

    #[test]
    fn planning_an_uninstall_changes_no_files() {
        let (runtime_paths, temp) = runtime_paths_with_temp_roots();
        full_install(Harness::Claude, InstallScope::Project, &runtime_paths);
        let installed = snapshot(temp.path());
        plan_uninstall_with_runtime(Harness::Claude, InstallScope::Project, &runtime_paths)
            .unwrap();
        assert_eq!(snapshot(temp.path()), installed);
    }

    #[test]
    fn uninstall_keeps_the_other_mcp_servers_in_the_config() {
        let (runtime_paths, _temp) = runtime_paths_with_temp_roots();
        full_install(Harness::Claude, InstallScope::Project, &runtime_paths);
        let config = runtime_paths.cwd.join(".mcp.json");
        let mut value: Value = serde_json::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
        value["mcpServers"]["github"] = json!({ "command": "github-mcp" });
        fs::write(&config, value.to_string()).unwrap();
        uninstall(Harness::Claude, InstallScope::Project, &runtime_paths);
        let left: Value = serde_json::from_str(&fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(
            left,
            json!({ "mcpServers": { "github": { "command": "github-mcp" } } })
        );
    }

    #[test]
    fn uninstall_keeps_the_users_own_codex_settings_next_to_codemods() {
        let (runtime_paths, _temp) = runtime_paths_with_temp_roots();
        let config = runtime_paths
            .codex_home
            .as_ref()
            .unwrap()
            .join("config.toml");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        fs::write(&config, "model = \"gpt-5\"\n").unwrap();
        full_install(Harness::Codex, InstallScope::User, &runtime_paths);
        uninstall(Harness::Codex, InstallScope::User, &runtime_paths);
        assert_eq!(fs::read_to_string(&config).unwrap(), "model = \"gpt-5\"\n");
    }

    #[test]
    fn uninstall_keeps_a_codemod_command_the_user_edited() {
        let (runtime_paths, _temp) = runtime_paths_with_temp_roots();
        full_install(Harness::Cursor, InstallScope::Project, &runtime_paths);
        let command = mcs_command_entrypoint_path_for_harness(
            Harness::Cursor,
            InstallScope::Project,
            &runtime_paths,
        )
        .unwrap()
        .unwrap();
        fs::write(&command, "Run my own migration checklist first.\n").unwrap();
        let plan = uninstall(Harness::Cursor, InstallScope::Project, &runtime_paths);
        assert_eq!(kept_paths(&plan), vec![command]);
    }

    #[test]
    fn uninstall_keeps_the_agents_md_section_another_set_up_agent_reads() {
        let (runtime_paths, _temp) = runtime_paths_with_temp_roots();
        full_install(Harness::Cursor, InstallScope::Project, &runtime_paths);
        full_install(Harness::Opencode, InstallScope::Project, &runtime_paths);
        uninstall(Harness::Cursor, InstallScope::Project, &runtime_paths);
        let guide = fs::read_to_string(runtime_paths.cwd.join(AGENTS_GUIDE_FILE_NAME)).unwrap();
        assert!(guide.contains(SKILL_DISCOVERY_SECTION_BEGIN));
    }

    #[test]
    fn uninstall_keeps_the_skill_folder_codex_and_antigravity_share() {
        let (runtime_paths, _temp) = runtime_paths_with_temp_roots();
        full_install(Harness::Codex, InstallScope::Project, &runtime_paths);
        full_install(Harness::Antigravity, InstallScope::Project, &runtime_paths);
        uninstall(Harness::Codex, InstallScope::Project, &runtime_paths);
        let skill =
            skills_root_for_harness(Harness::Antigravity, InstallScope::Project, &runtime_paths)
                .unwrap()
                .join(MCS_SKILL_DIR_NAME)
                .join(SKILL_FILE_NAME);
        assert!(skill.is_file());
    }

    #[test]
    fn uninstall_keeps_an_mcp_config_it_cannot_read() {
        let (runtime_paths, _temp) = runtime_paths_with_temp_roots();
        full_install(Harness::Cursor, InstallScope::Project, &runtime_paths);
        let config = runtime_paths.cwd.join(".cursor/mcp.json");
        fs::write(&config, "{ \"mcpServers\": { \"codemod\": ").unwrap();
        let plan = uninstall(Harness::Cursor, InstallScope::Project, &runtime_paths);
        assert_eq!(kept_paths(&plan), vec![config]);
    }

    #[test]
    fn uninstall_finds_nothing_to_remove_where_codemod_was_never_set_up() {
        let (runtime_paths, _temp) = runtime_paths_with_temp_roots();
        let plan =
            plan_uninstall_with_runtime(Harness::Claude, InstallScope::Project, &runtime_paths)
                .unwrap();
        assert!(plan.is_empty());
    }
}
