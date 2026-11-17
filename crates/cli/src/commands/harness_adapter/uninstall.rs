//! `codemod ai uninstall`: removes what `codemod ai` installed for one agent
//! and scope, and leaves anything that doesn't look like Codemod's own.

use super::doctor::{installed_setups, read_mcp_launch};
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

    /// Removes `files`, reported as one item at `shown`.
    fn remove_files(&mut self, what: &str, shown: &Path, files: Vec<PathBuf>) {
        self.record(what, shown);
        for file in files {
            self.changes.insert(file, PendingChange::DeleteFile);
        }
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
                    write_replacing(path, content).map_err(|error| failed(path, error))?
                }
                // `symlink_metadata` so a dangling symlink is removed too.
                PendingChange::DeleteFile if path.symlink_metadata().is_ok() => {
                    fs::remove_file(path).map_err(|error| failed(path, error))?
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

/// Writes beside the file and renames over it, so a crash never leaves a
/// half-written config such as `~/.claude.json`.
fn write_replacing(path: &Path, content: &str) -> std::io::Result<()> {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".codemod-uninstall");
    let temp = PathBuf::from(temp);
    fs::write(&temp, content)?;
    fs::rename(&temp, path).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })
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
    Ok(installed_setups(&runtime_paths, Some(scope))
        .into_iter()
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
    let other_setups = installed_setups(runtime_paths, Some(scope))
        .into_iter()
        .filter(|&(other, _)| other != harness)
        .collect::<Vec<_>>();

    plan_skill(&mut plan, &other_setups, runtime_paths)?;
    plan_command(&mut plan, runtime_paths)?;
    if harness_supports_mcp(harness) {
        let config_path = mcp_config_path_for_harness(harness, scope, runtime_paths)?;
        plan_mcp_entry(&mut plan, config_path);
    }
    if (harness, scope) == (Harness::Claude, InstallScope::User) {
        // Earlier releases wrote the Claude user server to `~/.mcp.json`.
        if let Some(home) = &runtime_paths.home_dir {
            plan_mcp_entry(&mut plan, home.join(".mcp.json"));
        }
    }
    plan_discovery_hints(&mut plan, &other_setups, runtime_paths)?;
    plan_periodic_update(&mut plan, runtime_paths)?;

    plan_install_record(&mut plan, runtime_paths)?;
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

/// A skill `codemod ai` installs: its label, folder name, the marker its
/// SKILL.md carries and the files install writes into the folder.
const INSTALLED_SKILLS: [(&str, &str, &str, &[&str]); 2] = [
    (
        "Codemod skill",
        MCS_SKILL_DIR_NAME,
        MCS_COMPATIBILITY_MARKER,
        &[SKILL_FILE_NAME],
    ),
    (
        "sdk-migration skill",
        SDK_MIGRATION_SKILL_DIR_NAME,
        "codemod-compatibility: skill-package-v1",
        &[
            SKILL_FILE_NAME,
            "commands/sdk-migration.md",
            "agents/openai.yaml",
        ],
    ),
];

fn plan_skill(
    plan: &mut UninstallPlan,
    other_setups: &[(Harness, InstallScope)],
    runtime_paths: &RuntimePaths,
) -> AdapterResult<()> {
    for (what, dir_name, marker, files) in INSTALLED_SKILLS {
        let skill_dir =
            skills_root_for_harness(plan.harness, plan.scope, runtime_paths)?.join(dir_name);
        let Some(content) = plan.read(&skill_dir.join(SKILL_FILE_NAME)) else {
            continue;
        };
        if !content.contains(marker) {
            plan.keep(what, &skill_dir, "it isn't the skill Codemod installs");
            continue;
        }
        // The skills folder can be a symlink to another agent's, so compare
        // the folders on disk, not the paths.
        let shared = shared_with(other_setups, |harness, scope| {
            skills_root_for_harness(harness, scope, runtime_paths)
                .is_ok_and(|root| same_existing_path(&root.join(dir_name), &skill_dir))
        });
        match shared {
            Some(other) => plan.keep(
                what,
                &skill_dir,
                format!("{} uses it too", other.display_name()),
            ),
            None => {
                let ours = files.iter().map(|file| skill_dir.join(file)).collect();
                remove_codemod_files(plan, what, &skill_dir, ours);
            }
        }
    }
    Ok(())
}

/// Removes the files and folders Codemod wrote inside `dir`, a folder it
/// created, and keeps `dir` when the user put anything else in it.
fn remove_codemod_files(plan: &mut UninstallPlan, what: &str, dir: &Path, ours: Vec<PathBuf>) {
    let theirs = files_under(dir)
        .iter()
        .any(|file| !ours.iter().any(|own| file.starts_with(own)));
    let files = ours
        .iter()
        .flat_map(|own| {
            if own.is_dir() {
                files_under(own)
            } else {
                vec![own.clone()]
            }
        })
        .filter(|file| file.is_file())
        .collect::<Vec<_>>();
    if files.is_empty() {
        return;
    }
    plan.remove_files(what, dir, files);
    if theirs {
        plan.keep(what, dir, "it has files you added, so they stay");
    }
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files
}

fn plan_install_record(
    plan: &mut UninstallPlan,
    runtime_paths: &RuntimePaths,
) -> AdapterResult<()> {
    let harness_root = harness_root_for_scope(plan.harness, plan.scope, runtime_paths)?;
    let ours = [
        CODEMOD_MANAGED_STATE_RELATIVE_PATH,
        CODEMOD_PERIODIC_UPDATE_RELATIVE_DIR,
    ]
    .map(|relative| harness_root.join(relative))
    .to_vec();
    remove_codemod_files(
        plan,
        "Codemod install record and update runner",
        &harness_root.join("codemod"),
        ours,
    );
    Ok(())
}

/// A slash command `codemod ai` installs: its label, file name and text.
const INSTALLED_COMMANDS: [(&str, &str, &str); 2] = [
    ("/codemod command", MCS_COMMAND_NAME, MCS_COMMAND_MD),
    (
        "/sdk-migration command",
        SDK_MIGRATION_SKILL_COMPONENT_ID,
        SDK_MIGRATION_COMMAND_MD,
    ),
];

/// A file is unchanged since install when it matches the fingerprint the
/// install record kept for it, which also covers text from older releases.
fn unchanged_since_install(
    plan: &UninstallPlan,
    path: &Path,
    runtime_paths: &RuntimePaths,
) -> bool {
    let Some(fingerprint) = content_fingerprint(path) else {
        return false;
    };
    managed_state_path_for_harness(plan.harness, plan.scope, runtime_paths)
        .ok()
        .and_then(|state_path| {
            read_managed_install_state_if_present(&state_path)
                .ok()
                .flatten()
        })
        .is_some_and(|state| {
            state.components.iter().any(|component| {
                Path::new(&component.path) == path
                    && component.fingerprint.as_deref() == Some(fingerprint.as_str())
            })
        })
}

fn plan_command(plan: &mut UninstallPlan, runtime_paths: &RuntimePaths) -> AdapterResult<()> {
    if plan.harness == Harness::Goose {
        if let Some((recipe_path, config_path)) =
            goose_mcs_command_paths(plan.scope, runtime_paths)?
        {
            plan_goose_command(plan, recipe_path, config_path);
        }
        return Ok(());
    }
    for (what, command_name, installed_text) in INSTALLED_COMMANDS {
        let Some(path) = mcs_command_entrypoint_path_for_harness(
            plan.harness,
            plan.scope,
            command_name,
            runtime_paths,
        )?
        else {
            continue;
        };
        match plan.read(&path) {
            Some(content)
                if content == installed_text
                    || unchanged_since_install(plan, &path, runtime_paths) =>
            {
                plan.remove_file(what, path)
            }
            Some(_) => plan.keep(what, &path, "it was edited"),
            None => {}
        }
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

/// Whether an MCP command line runs the Codemod CLI, directly or via npx.
fn launches_codemod(command: &[String]) -> bool {
    command.iter().any(|part| {
        part == CODEMOD_CLI_COMMAND
            || part.starts_with("codemod@")
            || Path::new(part).file_stem() == Some(CODEMOD_CLI_COMMAND.as_ref())
    })
}

fn plan_mcp_entry(plan: &mut UninstallPlan, config_path: PathBuf) {
    const WHAT: &str = "Codemod MCP server";
    let Some(content) = plan.read(&config_path) else {
        return;
    };
    match read_mcp_launch(plan.harness, &content) {
        Ok(Some(launch)) if launches_codemod(&launch.command) => {}
        Ok(Some(launch)) => {
            let command = launch.command.join(" ");
            let reason = format!("its `codemod` server runs `{command}`, not Codemod");
            return plan.keep(WHAT, &config_path, reason);
        }
        Ok(None) => return,
        Err(problem) => {
            let reason = format!("the file {problem}, so it was left as is");
            return plan.keep(WHAT, &config_path, reason);
        }
    }
    let edited = match plan.harness {
        Harness::Codex => remove_toml_mcp_entry(&content),
        Harness::Opencode => remove_opencode_mcp_entry(&content),
        _ => {
            let is_claude_state = config_path.file_name() == Some(".claude.json".as_ref());
            remove_json_mcp_entry(&content, !is_claude_state)
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

fn remove_json_mcp_entry(content: &str, delete_when_empty: bool) -> FileEdit {
    let mut config: Value =
        serde_json::from_str(content).map_err(|_| "isn't valid JSON".to_string())?;
    let Some(root) = config.as_object_mut() else {
        return Ok(None);
    };
    let Some(servers) = root.get_mut("mcpServers").and_then(Value::as_object_mut) else {
        return Ok(None);
    };
    if servers.remove(MCP_SERVER_NAME).is_none() {
        return Ok(None);
    }
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
        if !content.contains(SKILL_DISCOVERY_SECTION_BEGIN) {
            continue;
        }
        let Some(edited) = remove_managed_block(
            &content,
            SKILL_DISCOVERY_SECTION_BEGIN,
            SKILL_DISCOVERY_SECTION_END,
        ) else {
            let reason = "its end marker is missing, so remove the Codemod section by hand";
            plan.keep(WHAT, &guide, reason.to_string());
            continue;
        };
        let shared = shared_with(other_setups, |harness, scope| {
            discovery_guide_paths_with_runtime(harness, scope, runtime_paths)
                .is_ok_and(|paths| paths.iter().any(|path| same_existing_path(path, &guide)))
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
            MCS_COMMAND_NAME,
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

    fn codemod_skill_dir(harness: Harness, runtime_paths: &RuntimePaths) -> PathBuf {
        skills_root_for_harness(harness, InstallScope::Project, runtime_paths)
            .unwrap()
            .join(MCS_SKILL_DIR_NAME)
    }

    #[cfg(unix)]
    #[test]
    fn uninstall_keeps_a_skill_another_agent_reaches_through_a_symlinked_skills_folder() {
        let (runtime_paths, _temp) = runtime_paths_with_temp_roots();
        full_install(Harness::Cursor, InstallScope::Project, &runtime_paths);
        let cursor_skills =
            skills_root_for_harness(Harness::Cursor, InstallScope::Project, &runtime_paths)
                .unwrap();
        let claude_skills =
            skills_root_for_harness(Harness::Claude, InstallScope::Project, &runtime_paths)
                .unwrap();
        fs::create_dir_all(claude_skills.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&cursor_skills, &claude_skills).unwrap();
        full_install(Harness::Claude, InstallScope::Project, &runtime_paths);
        uninstall(Harness::Claude, InstallScope::Project, &runtime_paths);
        assert!(codemod_skill_dir(Harness::Cursor, &runtime_paths)
            .join(SKILL_FILE_NAME)
            .is_file());
    }

    #[test]
    fn uninstall_keeps_a_codemod_command_the_user_added_notes_to() {
        let (runtime_paths, _temp) = runtime_paths_with_temp_roots();
        full_install(Harness::Cursor, InstallScope::Project, &runtime_paths);
        let command = mcs_command_entrypoint_path_for_harness(
            Harness::Cursor,
            InstallScope::Project,
            MCS_COMMAND_NAME,
            &runtime_paths,
        )
        .unwrap()
        .unwrap();
        let mut content = fs::read_to_string(&command).unwrap();
        content.push_str("\nAlso run the linter afterwards.\n");
        fs::write(&command, &content).unwrap();
        uninstall(Harness::Cursor, InstallScope::Project, &runtime_paths);
        assert_eq!(fs::read_to_string(&command).unwrap(), content);
    }

    #[test]
    fn uninstall_keeps_files_the_user_added_to_codemods_folders() {
        let (runtime_paths, _temp) = runtime_paths_with_temp_roots();
        full_install(Harness::Claude, InstallScope::Project, &runtime_paths);
        let in_skill = codemod_skill_dir(Harness::Claude, &runtime_paths).join("my-notes.md");
        let in_record = runtime_paths.cwd.join(".claude/codemod/mine/notes.md");
        for file in [&in_skill, &in_record] {
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, "mine\n").unwrap();
        }
        uninstall(Harness::Claude, InstallScope::Project, &runtime_paths);
        assert!(in_skill.is_file() && in_record.is_file());
    }

    #[test]
    fn uninstall_keeps_a_codemod_mcp_server_that_runs_something_else() {
        let (runtime_paths, _temp) = runtime_paths_with_temp_roots();
        let config = runtime_paths.cwd.join(".mcp.json");
        let own = r#"{"mcpServers":{"codemod":{"command":"my-own-thing"}}}"#;
        fs::write(&config, own).unwrap();
        uninstall(Harness::Claude, InstallScope::Project, &runtime_paths);
        assert_eq!(fs::read_to_string(&config).unwrap(), own);
    }

    #[test]
    fn uninstall_reports_a_codemod_section_whose_end_marker_was_deleted() {
        let (runtime_paths, _temp) = runtime_paths_with_temp_roots();
        full_install(Harness::Cursor, InstallScope::Project, &runtime_paths);
        let guide = runtime_paths.cwd.join(AGENTS_GUIDE_FILE_NAME);
        let content = fs::read_to_string(&guide).unwrap();
        fs::write(&guide, content.replace(SKILL_DISCOVERY_SECTION_END, "")).unwrap();
        let plan = uninstall(Harness::Cursor, InstallScope::Project, &runtime_paths);
        assert_eq!(kept_paths(&plan), vec![guide]);
    }

    #[test]
    fn uninstall_for_all_projects_finds_the_setup_when_run_from_the_home_folder() {
        let (mut runtime_paths, _temp) = runtime_paths_with_temp_roots();
        runtime_paths.cwd = runtime_paths.home_dir.clone().unwrap();
        full_install(Harness::Claude, InstallScope::User, &runtime_paths);
        assert_eq!(
            installed_setups(&runtime_paths, Some(InstallScope::User)),
            vec![(Harness::Claude, InstallScope::User)]
        );
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
