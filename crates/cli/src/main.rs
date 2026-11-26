use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use inquire::validator::Validation;
use inquire::{Select, Text};
use std::fmt;
use std::io::{self, IsTerminal, Write};
use std::sync::Arc;
mod agent_log_renderer;
mod ascii_art;
mod assess_preflight;
mod auth;
mod auth_provider;
mod capabilities_security_callback;
mod commands;
mod control;
mod diagnostics;
mod dirty_git_check;
mod engine;
mod feedback;
mod journal;
mod launcher;
mod person_telemetry;
mod pro_dry_run;
mod progress_bar;
mod report_server;
mod suitability;
mod user_settings;
mod utils;
mod workflow_exec;
mod workflow_test;
use crate::auth::TokenStorage;
use ascii_art::print_ascii_art;
use codemod_telemetry::{
    send_composite::CompositeSender,
    send_event::{PostHogSender, TelemetrySender, TelemetrySenderOptions},
    send_null::NullSender,
    send_scarf::ScarfSender,
};

pub const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

pub(crate) fn experimental_notice(api: &str) -> String {
    format!("Experimental: {api} may change in a minor release.")
}

#[derive(Parser)]
#[command(name = "codemod", after_help = crate::launcher::HELP)]
#[command(
    about = "A self-hostable workflow engine for code transformations",
    long_about = "\x1b[32m      __                  __                                    __         \x1b[0m\n\x1b[32m     / /                 /\\ \\                                  /\\ \\        \x1b[0m\n\x1b[32m    / /   ___     ___    \\_\\ \\      __     ___ ___      ___    \\_\\ \\       \x1b[0m\n\x1b[32m   / /   /'___\\  / __`\\  /'_` \\   /'__`\\ /' __` __`\\   / __`\\  /'_` \\      \x1b[0m\n\x1b[32m  / /   /\\ \\__/ /\\ \\L\\ \\/\\ \\L\\ \\ /\\  __/ /\\ \\/\\ \\/\\ \\ /\\ \\L\\ \\/\\ \\L\\ \\  __ \x1b[0m\n\x1b[32m /_/    \\ \\____\\\\ \\____/\\ \\___,_\\\\ \\____\\\\ \\_\\ \\_\\ \\_\\\\ \\____/\\ \\___,_\\/\\_\\\x1b[0m\n\x1b[32m/_/      \\/____/ \\/___/  \\/__,_ / \\/____/ \\/_/\\/_/\\/_/ \\/___/  \\/__,_ /\\/_/\x1b[0m\n\x1b[32m                                                                           \x1b[0m\n\x1b[32m                                                                           \x1b[0m\n\nA self-hostable workflow engine for code transformations",
    version = CLI_VERSION
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Verbose output
    #[arg(short, long, global = true)]
    verbose: bool,

    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    trailing_args: Vec<String>,

    /// Disable telemetry
    #[arg(long, global = true, action = clap::ArgAction::SetTrue)]
    disable_analytics: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage workflows
    Workflow(WorkflowArgs),

    /// JavaScript ast-grep execution
    Jssg(JssgArgs),

    /// Initialize a new workflow
    Init(commands::init::Command),

    /// Login to a registry
    Login(commands::login::Command),

    /// Logout from a registry
    Logout(commands::logout::Command),

    /// Show current authentication status
    Whoami(commands::whoami::Command),

    /// Manage CLI telemetry preferences
    Telemetry(commands::telemetry::Command),

    /// Publish a workflow
    Publish(commands::publish::Command),

    /// Manage codemod package manifests
    Manifest(commands::manifest::Command),

    /// Search for packages in the registry
    Search(commands::search::Command),

    /// Run a codemod from the registry
    Run(commands::run::Command),

    /// Unpublish a package from the registry
    Unpublish(commands::unpublish::Command),

    /// Manage package cache
    Cache(commands::cache::Command),

    /// Install and manage Codemod AI integrations (experimental)
    Ai(commands::ai::Command),

    /// Start MCP (Model Context Protocol) server (experimental)
    Mcp(commands::mcp::Command),
}

#[derive(Args, Debug)]
struct WorkflowArgs {
    #[command(subcommand)]
    command: WorkflowCommands,
}

#[derive(Args, Debug)]
struct JssgArgs {
    #[command(subcommand)]
    command: JssgCommands,
}

#[derive(Subcommand, Debug)]
enum WorkflowCommands {
    /// Run a workflow
    Run(commands::workflow::run::Command),

    /// Resume a paused workflow
    Resume(commands::workflow::resume::Command),

    /// Validate a workflow file
    Validate(commands::workflow::validate::Command),

    /// Show workflow run status
    Status(commands::workflow::status::Command),

    /// List workflow runs
    List(commands::workflow::list::Command),

    /// Cancel a workflow run
    Cancel(commands::workflow::cancel::Command),

    /// Send an event to a workflow run that waits for it (experimental)
    Send(commands::workflow::send::Command),

    /// Print a run's events, then follow it while it runs elsewhere (experimental)
    Attach(commands::workflow::attach::Command),

    /// Describe a workflow without running it: params, imports, capabilities, steps (experimental)
    Inspect(commands::workflow::inspect::Command),

    /// Convert workflow.yaml files to workflow.ts
    Upgrade(commands::workflow::upgrade::Command),

    /// Run one workflow test from a JSON spec on stdin (used by `codemod/testing`)
    #[command(hide = true)]
    Harness(commands::workflow::harness::Command),
}

#[derive(Subcommand, Debug)]
enum JssgCommands {
    /// Bundle JavaScript/TypeScript files and dependencies
    Bundle(commands::jssg::bundle::Command),
    /// Run JavaScript code transformation
    Run(commands::jssg::run::Command),
    /// Test JavaScript code transformations
    Test(commands::jssg::test::Command),
    /// List applicable JavaScript code transformations
    ListApplicable(commands::jssg::list_applicable::Command),
    /// Execute a JavaScript file directly
    Exec(commands::jssg::exec::Command),
}

/// Check if a string looks like a package name that should be run
fn is_package_name(arg: &str) -> bool {
    if arg.starts_with('-') {
        return false;
    }

    // Check for scoped packages (@org/package)
    if arg.starts_with('@') && arg.contains('/') {
        return true;
    }

    // Check for package with version (@org/package@1.0.0 or package@1.0.0)
    if arg.contains('@') && !arg.starts_with('@') {
        return true;
    }

    // Check for simple package names (exclude known subcommands)
    let known_commands = [
        "workflow",
        "jssg",
        "init",
        "login",
        "logout",
        "whoami",
        "telemetry",
        "publish",
        "manifest",
        "search",
        "run",
        "unpublish",
        "cache",
        "ai",
        "mcp",
    ];

    !known_commands.contains(&arg)
}

type TelemetrySenderMutex = Arc<Box<dyn TelemetrySender + Send + Sync>>;

enum ImplicitRoute {
    Run(Vec<String>),
}

enum NoCommandResult {
    Completed,
    ExitWithMessage(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NoCommandAction {
    Ai,
    Init,
    RunPackage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NoCommandPromptOption {
    action: NoCommandAction,
    label: String,
}

impl fmt::Display for NoCommandPromptOption {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.label)
    }
}

/// The commands are pinned to this version: a bare `npx codemod` is
/// the `latest` tag, which is 1.x while 2.x is a prerelease.
fn no_command_prompt_options() -> Vec<NoCommandPromptOption> {
    use utils::cli_command::npx_codemod;
    vec![
        NoCommandPromptOption {
            action: NoCommandAction::Ai,
            label: format!("Install Master Codemod Skills ({})", npx_codemod("ai")),
        },
        NoCommandPromptOption {
            action: NoCommandAction::Init,
            label: format!("Create a new codemod package ({})", npx_codemod("init")),
        },
        NoCommandPromptOption {
            action: NoCommandAction::RunPackage,
            label: format!("Run a published package ({})", npx_codemod("<package>")),
        },
    ]
}

fn no_command_message() -> String {
    use utils::cli_command::npx_codemod;
    [
        "No command provided.".to_string(),
        String::new(),
        "Next steps:".to_string(),
        format!("  1. Install Master Codemod Skills: {}", npx_codemod("ai")),
        format!("  2. Create a new codemod package: {}", npx_codemod("init")),
        format!("  3. Run a published package: {}", npx_codemod("<package>")),
        String::new(),
        "Use --help for more usage information.".to_string(),
    ]
    .join("\n")
}

fn should_prompt_for_no_command_action() -> bool {
    crate::utils::interactive::is_interactive(false) && io::stdout().is_terminal()
}

fn exit_with_code(code: i32) -> ! {
    let _ = io::stdout().flush();
    let _ = io::stderr().flush();
    std::process::exit(code);
}

/// Effective analytics opt-out, including implicit `codemod @pkg … --disable-analytics`
/// where the flag lives in trailing args until the run subcommand is re-parsed.
/// `--disable-analytics` is an on/off flag (`ArgAction::SetTrue`), so a literal match
/// in trailing args is enough; re-parsing does not catch additional spellings.
fn effective_disable_analytics(cli: &Cli) -> bool {
    cli.disable_analytics
        || cli
            .trailing_args
            .iter()
            .any(|arg| arg == "--disable-analytics")
        || do_not_track(std::env::var("DO_NOT_TRACK").ok().as_deref())
}

/// The `DO_NOT_TRACK` convention (consoledonottrack.com): set to anything
/// but empty, `0` or `false`, it opts out like `DISABLE_ANALYTICS`.
fn do_not_track(value: Option<&str>) -> bool {
    value.map(str::trim).is_some_and(|value| {
        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    })
}

const PERSON_TRAITS_EXIT_GRACE: std::time::Duration = std::time::Duration::from_secs(1);

async fn await_person_traits_sync(handle: Option<tokio::task::JoinHandle<()>>) {
    if let Some(handle) = handle {
        let _ = tokio::time::timeout(PERSON_TRAITS_EXIT_GRACE, handle).await;
    }
}

/// Printed by the no-command path before returning; main should exit without
/// rendering the error again.
#[derive(Debug, thiserror::Error)]
#[error("No command provided")]
struct NoCommandAlreadyReported;

fn validate_no_command_package_input(
    input: &str,
) -> std::result::Result<Validation, inquire::CustomUserError> {
    if input.trim().is_empty() {
        Ok(Validation::Invalid("Please enter a package name.".into()))
    } else {
        Ok(Validation::Valid)
    }
}

fn normalize_no_command_package_input(input: String) -> String {
    input.trim().to_string()
}

fn dispatch_selected_init_command() -> Result<()> {
    let command = commands::init::Command::default();
    commands::init::handler(&command)
}

async fn dispatch_selected_ai_command(telemetry_sender: TelemetrySenderMutex) -> Result<()> {
    let cli = Cli::try_parse_from(["codemod", "ai"])?;
    match cli.command {
        Some(Commands::Ai(args)) => commands::ai::handler(&args, telemetry_sender).await,
        _ => Ok(()),
    }
}

async fn dispatch_selected_run_command(
    package: &str,
    telemetry_sender: TelemetrySenderMutex,
    disable_analytics: bool,
) -> Result<()> {
    let command = commands::run::Command::from_package_for_prompt(package.to_string());
    commands::run::handler(&command, telemetry_sender, disable_analytics).await
}

async fn handle_no_command(
    telemetry_sender: TelemetrySenderMutex,
    disable_analytics: bool,
) -> Result<NoCommandResult> {
    print_ascii_art();

    if !should_prompt_for_no_command_action() {
        return Ok(NoCommandResult::ExitWithMessage(no_command_message()));
    }

    let selection = Select::new("What would you like to do?", no_command_prompt_options())
        .with_starting_cursor(0)
        .prompt();

    let action = match selection {
        Ok(selection) => selection.action,
        Err(_) => return Ok(NoCommandResult::ExitWithMessage(no_command_message())),
    };

    let result = match action {
        NoCommandAction::Ai => dispatch_selected_ai_command(telemetry_sender).await,
        NoCommandAction::Init => dispatch_selected_init_command(),
        NoCommandAction::RunPackage => {
            let package = Text::new("Package name:")
                .with_help_message("Example: react/19/migration-recipe or @your-org/package")
                .with_validator(validate_no_command_package_input)
                .prompt()?;
            let package = normalize_no_command_package_input(package);
            dispatch_selected_run_command(&package, telemetry_sender, disable_analytics).await
        }
    };

    result.map(|_| NoCommandResult::Completed)
}

fn classify_implicit_route(trailing_args: &[String]) -> Option<ImplicitRoute> {
    if trailing_args.is_empty() {
        return None;
    }

    let package = trailing_args.first()?;
    if !is_package_name(package) {
        return None;
    }

    let mut run_args = vec!["codemod".to_string(), "run".to_string()];
    run_args.extend(trailing_args.iter().cloned());
    Some(ImplicitRoute::Run(run_args))
}

/// Handle implicit run command from trailing arguments
async fn handle_implicit_run_command(
    trailing_args: Vec<String>,
    telemetry_sender: TelemetrySenderMutex,
) -> Result<bool> {
    let Some(route) = classify_implicit_route(&trailing_args) else {
        return Ok(false);
    };

    // Re-parse the entire CLI with the run command included
    match route {
        ImplicitRoute::Run(full_args) => match Cli::try_parse_from(&full_args) {
            Ok(new_cli) => {
                if let Some(Commands::Run(run_args)) = new_cli.command {
                    commands::run::handler(
                        &run_args,
                        telemetry_sender.clone(),
                        new_cli.disable_analytics,
                    )
                    .await?;
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            Err(e) => {
                if e.kind() == clap::error::ErrorKind::UnknownArgument {
                    return Ok(false);
                }
                Ok(false)
            }
        },
    }
}

#[tokio::main]
async fn main() {
    if let Err(error) = run_cli().await {
        if let Some(failed) = error.downcast_ref::<workflow_exec::FinishedWithFailedFiles>() {
            if !failed.quiet {
                eprintln!("\n{}", console::style(failed).yellow().bold());
            }
            exit_with_code(workflow_exec::EXIT_FAILED_FILES);
        }
        if let Some(waiting) = error.downcast_ref::<workflow_exec::WaitingForInput>() {
            // The text output already showed the question and the command.
            if waiting.quiet {
                eprintln!("{waiting}");
            }
            exit_with_code(workflow_exec::EXIT_WAITING);
        }
        if let Some(step @ codemod_workflow::EngineError::InterruptedStep { .. }) =
            error.downcast_ref::<codemod_workflow::EngineError>()
        {
            eprintln!("{step}");
            exit_with_code(workflow_exec::EXIT_WAITING);
        }
        if let Some(failed) = error.downcast_ref::<workflow_exec::FailedByWorkflow>() {
            exit_with_code(failed.exit_code);
        }
        if error
            .downcast_ref::<workflow_exec::WorkflowFailureAlreadyReported>()
            .is_none()
            && error.downcast_ref::<NoCommandAlreadyReported>().is_none()
        {
            diagnostics::render_anyhow_error(&error);
        }
        exit_with_code(1);
    }
}

async fn run_cli() -> Result<()> {
    // Parse command line arguments
    let cli = Cli::parse();

    // Keep diagnostic logs quiet by default so workflow/TUI rendering owns the terminal.
    // Users can still opt into diagnostics with --verbose or RUST_LOG.
    let default_log_filter = if cli.verbose { "debug" } else { "error" };
    env_logger::init_from_env(env_logger::Env::default().default_filter_or(default_log_filter));

    let invocation_opt_out = effective_disable_analytics(&cli)
        || matches!(
            std::env::var("DISABLE_ANALYTICS").as_deref(),
            Ok("true" | "1")
        );
    if let Some(Commands::Telemetry(args)) = &cli.command {
        return commands::telemetry::handler(args, invocation_opt_out);
    }
    if !matches!(&cli.command, Some(Commands::Mcp(_)))
        && !matches!(
            &cli.command,
            Some(Commands::Workflow(WorkflowArgs {
                command: WorkflowCommands::Harness(_)
            }))
        )
    {
        commands::telemetry::maybe_print_notice();
    }

    let storage = (!invocation_opt_out).then(TokenStorage::new).transpose()?;
    let saved_opt_out = storage
        .as_ref()
        .map(TokenStorage::telemetry_disabled)
        .transpose()?
        .unwrap_or(false);
    if invocation_opt_out || saved_opt_out {
        std::env::set_var("DISABLE_ANALYTICS", "true");
    }

    let mut person_traits_task = None;
    let telemetry_sender: TelemetrySenderMutex =
        if let Some(storage) = storage.filter(|_| !saved_opt_out) {
            let config = storage.load_config()?;

            let auth = storage.get_auth_for_registry(&config.default_registry)?;

            let distinct_id = auth
                .as_ref()
                .map(|auth| auth.user.id.clone())
                .unwrap_or_else(|| {
                    storage
                        .get_or_create_anonymous_telemetry_id()
                        .unwrap_or_else(|error| {
                            log::debug!("Failed to persist anonymous telemetry id: {error}");
                            uuid::Uuid::new_v4().to_string()
                        })
                });

            let options = TelemetrySenderOptions {
                distinct_id,
                cloud_role: "CLI".to_string(),
            };

            let posthog = PostHogSender::new(options.clone())
                .await
                .inspect_err(|error| log::debug!("Failed to initialize PostHog telemetry: {error}"))
                .ok();
            let scarf = ScarfSender::new(options)
                .inspect_err(|error| log::debug!("Failed to initialize Scarf telemetry: {error}"))
                .ok();

            // Person traits go to PostHog directly, never through the composite:
            // Scarf would put the email in a URL query string. Anonymous
            // installs have no traits and are never aliased to a user.
            // Opt-out is resolved before this spawn (including implicit-run flag order).
            if let (Some(posthog), Some(auth)) = (&posthog, auth) {
                let posthog = posthog.clone();
                person_traits_task = Some(tokio::spawn(async move {
                    person_telemetry::sync_person_traits(&storage, &posthog, &auth.user).await;
                }));
            }

            // PostHog is the primary backend; Scarf is best-effort alongside it.
            // If PostHog fails to initialize, Scarf can still carry telemetry
            // on its own rather than disabling telemetry entirely.
            match (posthog, scarf) {
                (Some(posthog), scarf) => {
                    let mut sender = CompositeSender::new(Arc::new(posthog));
                    if let Some(scarf) = scarf {
                        sender = sender.with_secondary(Arc::new(scarf));
                    }
                    let sender: Box<dyn TelemetrySender + Send + Sync> = Box::new(sender);
                    Arc::new(sender)
                }
                (None, Some(scarf)) => Arc::new(Box::new(scarf)),
                (None, None) => Arc::new(Box::new(NullSender {})),
            }
        } else {
            Arc::new(Box::new(NullSender {}))
        };

    telemetry_sender.initialize_panic_telemetry().await;

    let result = dispatch_command(&cli, telemetry_sender).await;
    await_person_traits_sync(person_traits_task).await;

    result
}

async fn dispatch_command(cli: &Cli, telemetry_sender: TelemetrySenderMutex) -> Result<()> {
    match &cli.command {
        Some(Commands::Workflow(args)) => match &args.command {
            WorkflowCommands::Run(args) => {
                commands::workflow::run::handler(args, telemetry_sender.clone()).await?;
            }
            WorkflowCommands::Resume(args) => {
                commands::workflow::resume::handler(args, telemetry_sender.clone()).await?;
            }
            WorkflowCommands::Validate(args) => {
                commands::workflow::validate::handler(args)?;
            }
            WorkflowCommands::Status(args) => {
                commands::workflow::status::handler(args).await?;
            }
            WorkflowCommands::List(args) => {
                commands::workflow::list::handler(args).await?;
            }
            WorkflowCommands::Cancel(args) => {
                commands::workflow::cancel::handler(args).await?;
            }
            WorkflowCommands::Send(args) => {
                commands::workflow::send::handler(args).await?;
            }
            WorkflowCommands::Attach(args) => {
                commands::workflow::attach::handler(args).await?;
            }
            WorkflowCommands::Inspect(args) => {
                commands::workflow::inspect::handler(args).await?;
            }
            WorkflowCommands::Upgrade(args) => {
                commands::workflow::upgrade::handler(args)?;
            }
            WorkflowCommands::Harness(args) => {
                commands::workflow::harness::handler(args).await?;
            }
        },
        Some(Commands::Jssg(args)) => match &args.command {
            JssgCommands::Bundle(args) => {
                args.clone().run().await?;
            }
            JssgCommands::Run(args) => {
                commands::jssg::run::handler(args, telemetry_sender.clone()).await?;
            }
            JssgCommands::Test(args) => {
                commands::jssg::test::handler(args, telemetry_sender.clone()).await?;
            }
            JssgCommands::ListApplicable(args) => {
                commands::jssg::list_applicable::handler(args).await?;
            }
            JssgCommands::Exec(args) => {
                commands::jssg::exec::handler(args).await?;
            }
        },
        Some(Commands::Init(args)) => {
            commands::init::handler(args)?;
        }
        Some(Commands::Login(args)) => {
            commands::login::handler(args).await?;
        }
        Some(Commands::Logout(args)) => {
            commands::logout::handler(args).await?;
        }
        Some(Commands::Whoami(args)) => {
            commands::whoami::handler(args).await?;
        }
        Some(Commands::Telemetry(_)) => unreachable!("handled before initializing telemetry"),
        Some(Commands::Publish(args)) => {
            commands::publish::handler(args, telemetry_sender.clone()).await?;
        }
        Some(Commands::Manifest(args)) => {
            commands::manifest::handler(args)?;
        }
        Some(Commands::Search(args)) => {
            commands::search::handler(args).await?;
        }
        Some(Commands::Run(args)) => {
            commands::run::handler(args, telemetry_sender.clone(), cli.disable_analytics).await?;
        }
        Some(Commands::Unpublish(args)) => {
            commands::unpublish::handler(args).await?;
        }
        Some(Commands::Cache(args)) => {
            commands::cache::handler(args).await?;
        }
        Some(Commands::Ai(args)) => {
            commands::ai::handler(args, telemetry_sender.clone()).await?;
        }
        Some(Commands::Mcp(args)) => {
            args.run().await?;
        }
        None => {
            // Try to parse as implicit run command
            if !handle_implicit_run_command(cli.trailing_args.clone(), telemetry_sender.clone())
                .await?
            {
                match handle_no_command(telemetry_sender.clone(), cli.disable_analytics).await? {
                    NoCommandResult::Completed => {}
                    NoCommandResult::ExitWithMessage(message) => {
                        eprintln!("{message}");
                        return Err(NoCommandAlreadyReported.into());
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{error::ErrorKind, CommandFactory};

    #[test]
    fn top_level_help_lists_ai_and_mcp() {
        let help_text = Cli::command().render_long_help().to_string();
        assert!(help_text.contains("ai"));
        assert!(help_text.contains("mcp"));
    }

    #[test]
    fn no_command_message_lists_onboarding_steps() {
        let message = no_command_message();

        let ai_index = message
            .find(&format!(
                "1. Install Master Codemod Skills: npx codemod@{CLI_VERSION} ai"
            ))
            .expect("expected ai step");
        let init_index = message
            .find(&format!(
                "2. Create a new codemod package: npx codemod@{CLI_VERSION} init"
            ))
            .expect("expected init step");
        let package_index = message
            .find(&format!(
                "3. Run a published package: npx codemod@{CLI_VERSION} <package>"
            ))
            .expect("expected package step");

        assert!(ai_index < init_index);
        assert!(init_index < package_index);
    }

    #[test]
    fn no_command_prompt_options_list_ai_first() {
        let options = no_command_prompt_options();

        assert_eq!(options[0].action, NoCommandAction::Ai);
        assert_eq!(
            options[0].label,
            format!("Install Master Codemod Skills (npx codemod@{CLI_VERSION} ai)")
        );
        assert_eq!(options[1].action, NoCommandAction::Init);
        assert_eq!(
            options[1].label,
            format!("Create a new codemod package (npx codemod@{CLI_VERSION} init)")
        );
        assert_eq!(options[2].action, NoCommandAction::RunPackage);
    }

    #[test]
    fn validate_no_command_package_input_rejects_blank_values() {
        assert!(matches!(
            validate_no_command_package_input("").expect("blank validation"),
            Validation::Invalid(_)
        ));
        assert!(matches!(
            validate_no_command_package_input("   ").expect("whitespace validation"),
            Validation::Invalid(_)
        ));
        assert!(matches!(
            validate_no_command_package_input("@codemod/sample").expect("package validation"),
            Validation::Valid
        ));
    }

    #[test]
    fn normalize_no_command_package_input_trims_whitespace() {
        assert_eq!(
            normalize_no_command_package_input("  @codemod/sample  ".to_string()),
            "@codemod/sample"
        );
    }

    /// `npx codemod run pkg@ver --approve` failed with "unexpected argument":
    /// a registry run takes `workflow run`'s non-interactive flags.
    #[test]
    fn run_takes_the_non_interactive_flags_workflow_run_takes() {
        let flags = [
            "--approve",
            "--managed-git",
            "--dry-run",
            "--no-interactive",
            "--param",
            "a=b",
            "--allow-fs",
            "--allow-fetch",
            "--allow-child-process",
            "--allow-dirty",
        ];
        let workflow_run = Cli::try_parse_from(
            ["codemod", "workflow", "run", "-w", "workflow.ts"]
                .into_iter()
                .chain(flags),
        );
        assert!(workflow_run.is_ok(), "{:?}", workflow_run.err());

        let run_args = |cli: Cli| match cli.command {
            Some(Commands::Run(args)) => args,
            _ => panic!("not a run command"),
        };
        for prefix in [
            vec!["codemod", "run", "@org/pkg@1.0.0"],
            vec!["codemod", "@org/pkg@1.0.0"],
        ] {
            let args: Vec<String> = prefix.into_iter().chain(flags).map(String::from).collect();
            let full = match classify_implicit_route(&args[1..]) {
                Some(ImplicitRoute::Run(full)) if args[1] != "run" => full,
                _ => args,
            };
            let run = run_args(Cli::try_parse_from(&full).unwrap_or_else(|e| panic!("{e}")));
            assert_eq!(
                format!("{run:?}").matches(": true").count(),
                8,
                "approve, managed_git, dry_run, no_interactive, allow_fs, allow_fetch, allow_child_process, allow_dirty: {run:?}"
            );
        }
    }

    #[test]
    fn parser_accepts_ai_install_without_subcommand() {
        let parse_result = Cli::try_parse_from(["codemod", "ai"]);
        assert!(parse_result.is_ok());
    }

    #[test]
    fn parser_accepts_ai_feedback_opt_in() {
        let parse_result = Cli::try_parse_from(["codemod", "ai", "--allow-feedback"]);
        assert!(parse_result.is_ok());
    }

    #[test]
    fn parser_accepts_ai_feedback_command() {
        let parse_result = Cli::try_parse_from([
            "codemod",
            "ai",
            "feedback",
            "--category",
            "jssg",
            "--message",
            "Please add multi-file JSSG examples.",
        ]);
        assert!(parse_result.is_ok());
    }

    #[test]
    fn parser_accepts_ai_list() {
        let parse_result = Cli::try_parse_from(["codemod", "ai", "list"]);
        assert!(parse_result.is_ok());
    }

    #[test]
    fn parser_accepts_mcp_feedback_opt_in() {
        let parse_result = Cli::try_parse_from(["codemod", "mcp", "--allow-feedback"]);
        assert!(parse_result.is_ok());
    }

    #[test]
    fn parser_accepts_run_with_install_skill_override() {
        let parse_result = Cli::try_parse_from([
            "codemod",
            "run",
            "@codemod/sample",
            "--no-interactive",
            "--install-skill",
        ]);
        assert!(parse_result.is_ok());
    }

    #[test]
    fn parser_accepts_workflow_run_with_install_skill_override() {
        let parse_result = Cli::try_parse_from([
            "codemod",
            "workflow",
            "run",
            "--workflow",
            "workflow.yaml",
            "--no-interactive",
            "--install-skill",
        ]);
        assert!(parse_result.is_ok());
    }

    #[test]
    fn parser_accepts_workflow_resume_with_install_skill_override() {
        let parse_result = Cli::try_parse_from([
            "codemod",
            "workflow",
            "resume",
            "--workflow",
            "workflow.yaml",
            "--id",
            "00000000-0000-0000-0000-000000000001",
            "--trigger-all",
            "--no-interactive",
            "--install-skill",
        ]);
        assert!(parse_result.is_ok());
    }

    #[test]
    fn classify_implicit_route_ignores_non_package_keyword() {
        let trailing_args = vec![
            "workflow".to_string(),
            "install".to_string(),
            "jest-to-vitest".to_string(),
        ];
        let route = classify_implicit_route(&trailing_args);
        assert!(route.is_none());
    }

    #[test]
    fn is_package_name_rejects_flag_like_values() {
        assert!(!is_package_name("--flag"));
        assert!(!is_package_name("--dry-run"));
    }

    #[test]
    fn classify_implicit_route_uses_run_when_skill_flag_absent() {
        let trailing_args = vec!["jest-to-vitest".to_string(), "--dry-run".to_string()];
        let route = classify_implicit_route(&trailing_args);
        assert!(matches!(route, Some(ImplicitRoute::Run(_))));
    }

    #[test]
    fn effective_disable_analytics_sees_flag_after_implicit_package_args() {
        // `codemod @pkg --dry-run --disable-analytics` parks the opt-out in trailing_args.
        let cli = Cli::try_parse_from([
            "codemod",
            "@codemod/sample",
            "--dry-run",
            "--disable-analytics",
        ])
        .expect("implicit package parse");
        assert!(!cli.disable_analytics);
        assert!(effective_disable_analytics(&cli));
    }

    #[test]
    fn do_not_track_opts_out_unless_empty_0_or_false() {
        for value in ["1", "true", "yes"] {
            assert!(do_not_track(Some(value)), "{value}");
        }
        for value in [None, Some(""), Some("0"), Some("false"), Some("FALSE")] {
            assert!(!do_not_track(value), "{value:?}");
        }
    }

    #[test]
    fn parser_accepts_ai_install_with_opencode_harness() {
        let parse_result = Cli::try_parse_from(["codemod", "ai", "--harness", "opencode"]);
        assert!(parse_result.is_ok());
    }

    #[test]
    fn parser_accepts_ai_install_with_cursor_harness() {
        let parse_result = Cli::try_parse_from(["codemod", "ai", "--harness", "cursor"]);
        assert!(parse_result.is_ok());
    }

    #[test]
    fn parser_accepts_ai_install_with_codex_harness() {
        let parse_result = Cli::try_parse_from(["codemod", "ai", "--harness", "codex"]);
        assert!(parse_result.is_ok());
    }

    #[test]
    fn parser_accepts_ai_install_with_antigravity_harness() {
        let parse_result = Cli::try_parse_from(["codemod", "ai", "--harness", "antigravity"]);
        assert!(parse_result.is_ok());
    }

    #[test]
    fn parser_accepts_ai_install_with_no_interactive() {
        let parse_result = Cli::try_parse_from(["codemod", "ai", "--no-interactive"]);
        assert!(parse_result.is_ok());
    }

    #[test]
    fn parser_accepts_ai_install_with_manual_update_policy() {
        let parse_result = Cli::try_parse_from(["codemod", "ai", "--update-policy", "manual"]);
        assert!(parse_result.is_ok());
    }

    #[test]
    fn parser_accepts_ai_install_with_runtime_update_policy_flags() {
        let parse_result = Cli::try_parse_from([
            "codemod",
            "ai",
            "--update-policy",
            "notify",
            "--update-source",
            "registry",
            "--require-signed-manifest",
        ]);
        assert!(parse_result.is_ok());
    }

    #[test]
    fn parser_accepts_ai_install_with_logs_format() {
        let parse_result = Cli::try_parse_from(["codemod", "ai", "--format", "logs"]);
        assert!(parse_result.is_ok());
    }

    #[test]
    fn parser_accepts_ai_update_with_logs_format() {
        let parse_result = Cli::try_parse_from(["codemod", "ai", "update", "--format", "logs"]);
        assert!(parse_result.is_ok());
    }

    #[test]
    fn parser_accepts_ai_list_with_logs_format() {
        let parse_result = Cli::try_parse_from(["codemod", "ai", "list", "--format", "logs"]);
        assert!(parse_result.is_ok());
    }

    #[test]
    fn parser_accepts_init_skill_flag() {
        let parse_result = Cli::try_parse_from([
            "codemod",
            "init",
            "my-skill",
            "--no-interactive",
            "--skill",
            "--language",
            "typescript",
            "--description",
            "Skill package",
            "--author",
            "Author <author@example.com>",
            "--license",
            "MIT",
        ]);
        assert!(parse_result.is_ok());
    }

    #[test]
    fn parser_rejects_init_with_skill_flag() {
        let parse_result = Cli::try_parse_from(["codemod", "init", "my-codemod", "--with-skill"]);
        assert!(parse_result.is_err());
    }

    #[test]
    fn ai_help_lists_update_and_list_only() {
        let parse_result = Cli::try_parse_from(["codemod", "ai", "--help"]);
        let error = match parse_result {
            Err(error) => error,
            Ok(_) => panic!("expected --help to return clap display help"),
        };
        assert_eq!(error.kind(), ErrorKind::DisplayHelp);

        let help_text = error.to_string();
        assert!(help_text.contains("--harness"));
        assert!(help_text.contains("update"));
        assert!(help_text.contains("list"));
    }

    #[test]
    fn install_help_lists_opencode_and_cursor_harnesses() {
        let parse_result = Cli::try_parse_from(["codemod", "ai", "--help"]);
        let error = match parse_result {
            Err(error) => error,
            Ok(_) => panic!("expected --help to return clap display help"),
        };
        assert_eq!(error.kind(), ErrorKind::DisplayHelp);

        let help_text = error.to_string();
        assert!(help_text.contains("opencode"));
        assert!(help_text.contains("cursor"));
        assert!(help_text.contains("codex"));
        assert!(!help_text.contains("antigravity"));
        assert!(help_text.contains("--no-interactive"));
        assert!(help_text.contains("--update-policy"));
        assert!(help_text.contains("--update-source"));
        assert!(help_text.contains("--require-signed-manifest"));
        assert!(help_text.contains("auto-safe"));
        assert!(help_text.contains("logs"));
    }
}
