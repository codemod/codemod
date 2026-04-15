use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use inquire::validator::Validation;
use inquire::{Select, Text};
use std::fmt;
use std::io::{self, IsTerminal, Write};
use std::sync::Arc;
mod agent_select;
mod ascii_art;
mod auth;
mod auth_provider;
mod capabilities_security_callback;
mod commands;
mod dirty_git_check;
mod engine;
mod progress_bar;
mod report_server;
mod suitability;
mod utils;
mod workflow_runner;
use crate::auth::TokenStorage;
use ascii_art::print_ascii_art;
use codemod_telemetry::{
    send_event::{PostHogSender, TelemetrySender, TelemetrySenderOptions},
    send_null::NullSender,
};

pub const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser)]
#[command(name = "codemod")]
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

    /// Publish a workflow
    Publish(commands::publish::Command),

    /// Search for packages in the registry
    Search(commands::search::Command),

    /// Run a codemod from the registry
    Run(commands::run::Command),

    /// Unpublish a package from the registry
    Unpublish(commands::unpublish::Command),

    /// Manage package cache
    Cache(commands::cache::Command),

    /// Install and manage Codemod AI integrations
    Ai(commands::ai::Command),

    /// Start MCP (Model Context Protocol) server
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
        "publish",
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NoCommandPromptOption {
    action: NoCommandAction,
    label: &'static str,
}

impl fmt::Display for NoCommandPromptOption {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label)
    }
}

fn no_command_prompt_options() -> Vec<NoCommandPromptOption> {
    vec![
        NoCommandPromptOption {
            action: NoCommandAction::Ai,
            label: "Install Master Codemod Skills (npx codemod ai)",
        },
        NoCommandPromptOption {
            action: NoCommandAction::Init,
            label: "Create a new codemod package (npx codemod init)",
        },
        NoCommandPromptOption {
            action: NoCommandAction::RunPackage,
            label: "Run a published package (npx codemod <package>)",
        },
    ]
}

fn no_command_message() -> String {
    [
        "No command provided.",
        "",
        "Next steps:",
        "  1. Install Master Codemod Skills: npx codemod ai",
        "  2. Create a new codemod package: npx codemod init",
        "  3. Run a published package: npx codemod <package>",
        "",
        "Use --help for more usage information.",
    ]
    .join("\n")
}

fn should_prompt_for_no_command_action() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

fn exit_with_code(code: i32) -> ! {
    let _ = io::stdout().flush();
    let _ = io::stderr().flush();
    std::process::exit(code);
}

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
async fn main() -> Result<()> {
    // Initialize logger
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("error"));

    // Parse command line arguments
    let cli = Cli::parse();

    // Set log level based on verbose flag
    if cli.verbose {
        std::env::set_var("RUST_LOG", "debug");
    } else {
        std::env::set_var("RUST_LOG", "info");
    }

    let implicit_cli_params = Cli::try_parse_from(cli.trailing_args.clone());

    if cli.disable_analytics
        || implicit_cli_params
            .map(|params| params.disable_analytics)
            .unwrap_or(false)
    {
        std::env::set_var("DISABLE_ANALYTICS", "true");
    }

    let telemetry_sender: Arc<Box<dyn TelemetrySender + Send + Sync>> =
        if std::env::var("DISABLE_ANALYTICS") == Ok("true".to_string())
            || std::env::var("DISABLE_ANALYTICS") == Ok("1".to_string())
        {
            Arc::new(Box::new(NullSender {}))
        } else {
            let storage = TokenStorage::new()?;
            let config = storage.load_config()?;

            let auth = storage.get_auth_for_registry(&config.default_registry)?;

            let distinct_id = auth
                .map(|auth| auth.user.id)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

            Arc::new(Box::new(
                PostHogSender::new(TelemetrySenderOptions {
                    distinct_id,
                    cloud_role: "CLI".to_string(),
                })
                .await,
            ))
        };

    telemetry_sender.initialize_panic_telemetry().await;

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
        Some(Commands::Publish(args)) => {
            commands::publish::handler(args, telemetry_sender.clone()).await?;
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
                        exit_with_code(1);
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
            .find("1. Install Master Codemod Skills: npx codemod ai")
            .expect("expected ai step");
        let init_index = message
            .find("2. Create a new codemod package: npx codemod init")
            .expect("expected init step");
        let package_index = message
            .find("3. Run a published package: npx codemod <package>")
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
            "Install Master Codemod Skills (npx codemod ai)"
        );
        assert_eq!(options[1].action, NoCommandAction::Init);
        assert_eq!(
            options[1].label,
            "Create a new codemod package (npx codemod init)"
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

    #[test]
    fn parser_accepts_ai_install_without_subcommand() {
        let parse_result = Cli::try_parse_from(["codemod", "ai"]);
        assert!(parse_result.is_ok());
    }

    #[test]
    fn parser_accepts_ai_list() {
        let parse_result = Cli::try_parse_from(["codemod", "ai", "list"]);
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
    fn parser_accepts_init_with_skill_flag() {
        let parse_result = Cli::try_parse_from([
            "codemod",
            "init",
            "my-with-skill",
            "--no-interactive",
            "--project-type",
            "ast-grep-js",
            "--with-skill",
            "--language",
            "typescript",
            "--description",
            "With skill package",
            "--author",
            "Author <author@example.com>",
            "--license",
            "MIT",
            "--package-manager",
            "npm",
        ]);
        assert!(parse_result.is_ok());
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
