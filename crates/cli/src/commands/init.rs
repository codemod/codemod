use crate::utils::skill_layout::{
    expected_authored_skill_relative_file, AGENTS_SKILL_ROOT_RELATIVE_PATH,
};
use anyhow::{anyhow, Context, Result};
use clap::Args;
use console::{style, Emoji};
use inquire::{Confirm, Select, Text};
use log::info;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

#[derive(Args, Debug, Default)]
#[command(after_help = INIT_AFTER_HELP)]
pub struct Command {
    /// Project directory name (required with --no-interactive)
    #[arg(value_name = "PATH")]
    path: Option<PathBuf>,

    /// Project name (defaults to directory name)
    #[arg(long)]
    name: Option<String>,

    /// Git repository URL
    #[arg(long)]
    git_repository_url: Option<String>,

    /// Project type (required with --no-interactive unless --skill is set)
    #[arg(long)]
    project_type: Option<ProjectType>,

    /// Scaffold a skill-focused package with an install-skill workflow
    #[arg(long, conflicts_with = "with_skill")]
    skill: bool,

    /// Also scaffold skill behavior alongside workflow files
    #[arg(long, conflicts_with = "skill")]
    with_skill: bool,

    /// Package manager (required with --no-interactive for workflow packages, i.e. any --project-type, or with --workspace)
    #[arg(long)]
    package_manager: Option<String>,

    /// Target language (required with --no-interactive)
    #[arg(long)]
    language: Option<String>,

    /// Project description (required with --no-interactive)
    #[arg(long)]
    description: Option<String>,

    /// Author name and email (required with --no-interactive)
    #[arg(long)]
    author: Option<String>,

    /// License (required with --no-interactive)
    #[arg(long)]
    license: Option<String>,

    /// Make package private
    #[arg(long)]
    private: bool,

    /// Overwrite existing files
    #[arg(long)]
    force: bool,

    /// Skip prompts; required values must then be passed as flags (see below)
    #[arg(long)]
    no_interactive: bool,

    /// Create GitHub Actions workflow for publishing
    #[arg(long)]
    github_action: bool,

    /// Create a monorepo workspace structure
    #[arg(long)]
    workspace: bool,
}

const INIT_AFTER_HELP: &str = "\
Non-interactive mode (--no-interactive) requires:
  PATH, --language, --description, --author, --license
  --project-type                  for workflow packages (omit with --skill)
  --package-manager               for workflow packages (any --project-type, including legacy
                                  shell/ast-grep-yaml), or with --workspace

Example:
  codemod init my-codemod --no-interactive --project-type ast-grep-js \\
    --package-manager npm --language typescript \\
    --description \"Migrate X to Y\" --author \"Jane Doe <jane@example.com>\" --license MIT";

#[derive(clap::ValueEnum, Clone, Debug, PartialEq)]
enum ProjectType {
    /// JavaScript ast-grep codemod
    AstGrepJs,
    /// Multi-step workflow: Shell + YAML + jssg
    Hybrid,
    /// Shell command workflow codemod (legacy)
    Shell,
    /// YAML ast-grep codemod (legacy)
    AstGrepYaml,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InteractiveCodemodType {
    Jssg,
    MultiStepWorkflow,
    AgentSkill,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PackageBehavior {
    WorkflowOnly,
    SkillOnly,
    WorkflowAndSkill,
}

impl PackageBehavior {
    fn includes_workflow(self) -> bool {
        matches!(self, Self::WorkflowOnly | Self::WorkflowAndSkill)
    }

    fn includes_skill(self) -> bool {
        matches!(self, Self::SkillOnly | Self::WorkflowAndSkill)
    }
}

struct ProjectConfig {
    name: String,
    description: String,
    author: String,
    license: String,
    project_type: ProjectType,
    package_behavior: PackageBehavior,
    language: String,
    private: bool,
    package_manager: Option<String>,
    git_repository_url: Option<String>,
    github_action: bool,
    workspace: bool,
}

// Template constants using include_str!
const CODEMOD_TEMPLATE: &str = include_str!("../templates/codemod.yaml");
const SKILL_CODEMOD_TEMPLATE: &str = include_str!("../templates/skill/codemod.yaml");
const SHELL_WORKFLOW_TEMPLATE: &str = include_str!("../templates/shell/workflow.yaml");
const ASTGREP_YAML_WORKFLOW_TEMPLATE: &str =
    include_str!("../templates/astgrep-yaml/workflow.yaml");
const SKILL_WORKFLOW_TEMPLATE: &str = include_str!("../templates/skill/workflow.yaml");
const GITIGNORE_TEMPLATE: &str = include_str!("../templates/common/.gitignore");
const README_TEMPLATE: &str = include_str!("../templates/common/README.md");
const SKILL_README_TEMPLATE: &str = include_str!("../templates/skill/README.md");
const WORKSPACE_SKILL_ROOT_README_TEMPLATE: &str =
    include_str!("../templates/common/workspace-skill-root-README.md");
const GITHUB_ACTION_TEMPLATE: &str = include_str!("../templates/common/publish.yml");
const GITHUB_ACTION_WORKSPACE_TEMPLATE: &str =
    include_str!("../templates/common/publish-workspace.yml");
const SKILL_TEMPLATE: &str = include_str!("../templates/skill/SKILL.md");
const SKILL_REFERENCES_INDEX_TEMPLATE: &str =
    include_str!("../templates/skill/references/index.md");
const SKILL_REFERENCES_USAGE_TEMPLATE: &str =
    include_str!("../templates/skill/references/usage.md");
const INSTALL_SKILL_NODE_TEMPLATE: &str = r#"

  - id: install-package-skill
    name: Install Package Skill
    type: automatic
    steps:
      - id: install-package-skill
        name: Install package skill
        install-skill:
          package: "{name}"
          path: "{skill_path}"
"#;

// Shell project templates
const SHELL_SETUP_SCRIPT: &str = include_str!("../templates/shell/scripts/setup.sh");
const SHELL_TRANSFORM_SCRIPT: &str = include_str!("../templates/shell/scripts/transform.sh");
const SHELL_CLEANUP_SCRIPT: &str = include_str!("../templates/shell/scripts/cleanup.sh");

// JS ast-grep project templates
const JS_PACKAGE_JSON_TEMPLATE: &str = include_str!("../templates/js-astgrep/package.json");

/// The workflow.ts of a JSSG package, by language: a `forEachFile` callback
/// that parses each file and rewrites it.
fn callback_workflow_template(language: &str) -> &'static str {
    match language {
        "typescript" => include_str!("../templates/js-astgrep/workflows/typescript.ts"),
        "python" => include_str!("../templates/js-astgrep/workflows/python.ts"),
        "rust" => include_str!("../templates/js-astgrep/workflows/rust.ts"),
        "go" => include_str!("../templates/js-astgrep/workflows/go.ts"),
        "java" => include_str!("../templates/js-astgrep/workflows/java.ts"),
        "html" => include_str!("../templates/js-astgrep/workflows/html.ts"),
        "xml" => include_str!("../templates/js-astgrep/workflows/xml.ts"),
        "css" => include_str!("../templates/js-astgrep/workflows/css.ts"),
        "kotlin" => include_str!("../templates/js-astgrep/workflows/kotlin.ts"),
        "csharp" => include_str!("../templates/js-astgrep/workflows/csharp.ts"),
        "cpp" => include_str!("../templates/js-astgrep/workflows/cpp.ts"),
        "c" => include_str!("../templates/js-astgrep/workflows/c.ts"),
        "php" => include_str!("../templates/js-astgrep/workflows/php.ts"),
        "ruby" => include_str!("../templates/js-astgrep/workflows/ruby.ts"),
        "elixir" => include_str!("../templates/js-astgrep/workflows/elixir.ts"),
        "json" => include_str!("../templates/js-astgrep/workflows/json.ts"),
        "yaml" => include_str!("../templates/js-astgrep/workflows/yaml.ts"),
        "toml" => include_str!("../templates/js-astgrep/workflows/toml.ts"),
        _ => include_str!("../templates/js-astgrep/workflows/javascript.ts"),
    }
}
const JS_TSCONFIG_TEMPLATE: &str = include_str!("../templates/js-astgrep/tsconfig.json");

// fixtures
const JS_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.js");
const JS_TEST_EXPECTED: &str = include_str!("../templates/js-astgrep/tests/fixtures/expected.js");
const GO_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.go");
const GO_TEST_EXPECTED: &str = include_str!("../templates/js-astgrep/tests/fixtures/expected.go");
const PYTHON_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.py");
const PYTHON_TEST_EXPECTED: &str =
    include_str!("../templates/js-astgrep/tests/fixtures/expected.py");
const RUST_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.rs");
const RUST_TEST_EXPECTED: &str = include_str!("../templates/js-astgrep/tests/fixtures/expected.rs");
const JAVA_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.java");
const JAVA_TEST_EXPECTED: &str =
    include_str!("../templates/js-astgrep/tests/fixtures/expected.java");
const HTML_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.html");
const HTML_TEST_EXPECTED: &str =
    include_str!("../templates/js-astgrep/tests/fixtures/expected.html");
const XML_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.xml");
const XML_TEST_EXPECTED: &str = include_str!("../templates/js-astgrep/tests/fixtures/expected.xml");
const CSS_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.css");
const CSS_TEST_EXPECTED: &str = include_str!("../templates/js-astgrep/tests/fixtures/expected.css");
const KOTLIN_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.kt");
const KOTLIN_TEST_EXPECTED: &str =
    include_str!("../templates/js-astgrep/tests/fixtures/expected.kt");
const CSHARP_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.cs");
const CSHARP_TEST_EXPECTED: &str =
    include_str!("../templates/js-astgrep/tests/fixtures/expected.cs");
const CPP_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.cpp");
const CPP_TEST_EXPECTED: &str = include_str!("../templates/js-astgrep/tests/fixtures/expected.cpp");
const C_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.c");
const C_TEST_EXPECTED: &str = include_str!("../templates/js-astgrep/tests/fixtures/expected.c");
const PHP_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.php");
const PHP_TEST_EXPECTED: &str = include_str!("../templates/js-astgrep/tests/fixtures/expected.php");
const RUBY_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.rb");
const RUBY_TEST_EXPECTED: &str = include_str!("../templates/js-astgrep/tests/fixtures/expected.rb");
const ELIXIR_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.ex");
const ELIXIR_TEST_EXPECTED: &str =
    include_str!("../templates/js-astgrep/tests/fixtures/expected.ex");
const JSON_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.json");
const JSON_TEST_EXPECTED: &str =
    include_str!("../templates/js-astgrep/tests/fixtures/expected.json");
const YAML_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.yaml");
const YAML_TEST_EXPECTED: &str =
    include_str!("../templates/js-astgrep/tests/fixtures/expected.yaml");
const TOML_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.toml");
const TOML_TEST_EXPECTED: &str =
    include_str!("../templates/js-astgrep/tests/fixtures/expected.toml");

// ast-grep YAML project templates
const ASTGREP_PATTERNS_FOR_JAVASCRIPT: &str =
    include_str!("../templates/astgrep-yaml/rules/config.ts.yml");
const ASTGREP_PATTERNS_FOR_PYTHON: &str =
    include_str!("../templates/astgrep-yaml/rules/config.py.yml");
const ASTGREP_PATTERNS_FOR_RUST: &str =
    include_str!("../templates/astgrep-yaml/rules/config.rs.yml");
const ASTGREP_PATTERNS_FOR_GO: &str = include_str!("../templates/astgrep-yaml/rules/config.go.yml");
const ASTGREP_PATTERNS_FOR_JAVA: &str =
    include_str!("../templates/astgrep-yaml/rules/config.java.yml");
const ASTGREP_PATTERNS_FOR_HTML: &str =
    include_str!("../templates/astgrep-yaml/rules/config.html.yml");
const ASTGREP_PATTERNS_FOR_XML: &str =
    include_str!("../templates/astgrep-yaml/rules/config.xml.yml");
const ASTGREP_PATTERNS_FOR_CSS: &str =
    include_str!("../templates/astgrep-yaml/rules/config.css.yml");
const ASTGREP_PATTERNS_FOR_KOTLIN: &str =
    include_str!("../templates/astgrep-yaml/rules/config.kt.yml");
const ASTGREP_PATTERNS_FOR_CSHARP: &str =
    include_str!("../templates/astgrep-yaml/rules/config.cs.yml");
const ASTGREP_PATTERNS_FOR_CPP: &str =
    include_str!("../templates/astgrep-yaml/rules/config.cpp.yml");
const ASTGREP_PATTERNS_FOR_C: &str = include_str!("../templates/astgrep-yaml/rules/config.c.yml");
const ASTGREP_PATTERNS_FOR_PHP: &str =
    include_str!("../templates/astgrep-yaml/rules/config.php.yml");
const ASTGREP_PATTERNS_FOR_RUBY: &str =
    include_str!("../templates/astgrep-yaml/rules/config.rb.yml");
const ASTGREP_PATTERNS_FOR_ELIXIR: &str =
    include_str!("../templates/astgrep-yaml/rules/config.ex.yml");
const ASTGREP_PATTERNS_FOR_JSON: &str =
    include_str!("../templates/astgrep-yaml/rules/config.json.yml");
const ASTGREP_PATTERNS_FOR_YAML: &str =
    include_str!("../templates/astgrep-yaml/rules/config.yaml.yml");

static ROCKET: Emoji<'_, '_> = Emoji("🚀 ", "");
static CHECKMARK: Emoji<'_, '_> = Emoji("✓ ", "");

pub fn handler(args: &Command) -> Result<()> {
    let git_repository_url = args.git_repository_url.clone();

    let (project_path, project_name, git_repository_url) = if args.no_interactive {
        let project_path = match args.path.clone() {
            Some(path) => path,
            None => return Err(anyhow!("Path argument is required")),
        };

        let project_name = match args.name.clone() {
            Some(name) => name,
            None => {
                let file_name = project_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| {
                        anyhow!(
                            "Could not determine project name from path {}",
                            project_path.display()
                        )
                    })?;
                file_name.to_string()
            }
        };

        (project_path, project_name, git_repository_url)
    } else {
        // Interactive mode - ask for path if not provided
        let project_path = if let Some(path) = &args.path {
            path.clone()
        } else {
            let path_str = Text::new("Project directory:")
                .with_default("my-codemod")
                .prompt()?;
            PathBuf::from(path_str)
        };

        let project_name = args.name.clone().unwrap_or_else(|| {
            project_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("my-codemod")
                .to_string()
        });

        (project_path, project_name, git_repository_url)
    };

    if project_path.exists() && !args.force {
        return Err(anyhow!(
            "Directory already exists: {}. Use --force to overwrite.",
            project_path.display()
        ));
    }

    let config = if args.no_interactive {
        let package_behavior = package_behavior_from_flags(args.skill, args.with_skill)?;
        if package_behavior == PackageBehavior::SkillOnly && args.project_type.is_some() {
            return Err(anyhow!(
                "--project-type cannot be used with --skill. Remove --project-type for skill-only scaffolding."
            ));
        }

        let project_type = if package_behavior.includes_workflow() {
            let selected_project_type = args
                .project_type
                .clone()
                .ok_or_else(|| anyhow!("Project type is required --project-type"))?;
            normalize_project_type(selected_project_type)
        } else {
            // Skill-only packages do not scaffold workflow project assets.
            ProjectType::AstGrepJs
        };

        let package_manager = match (
            package_behavior,
            &project_type,
            args.package_manager.clone(),
            args.workspace,
        ) {
            (
                PackageBehavior::WorkflowOnly | PackageBehavior::WorkflowAndSkill,
                ProjectType::AstGrepJs,
                Some(pm),
                _,
            )
            | (
                PackageBehavior::WorkflowOnly | PackageBehavior::WorkflowAndSkill,
                ProjectType::Hybrid,
                Some(pm),
                _,
            )
            | (_, _, Some(pm), true) => Some(pm),
            (
                PackageBehavior::WorkflowOnly | PackageBehavior::WorkflowAndSkill,
                ProjectType::AstGrepJs,
                None,
                _,
            )
            | (
                PackageBehavior::WorkflowOnly | PackageBehavior::WorkflowAndSkill,
                ProjectType::Hybrid,
                None,
                _,
            ) => {
                return Err(anyhow!(
                    "--package-manager is required for workflow packages (any --project-type, including legacy shell/ast-grep-yaml)"
                ));
            }
            (_, _, None, true) => {
                return Err(anyhow!(
                    "--package-manager is required when --workspace is enabled"
                ));
            }
            _ => None,
        };
        ProjectConfig {
            name: project_name,
            description: args
                .description
                .clone()
                .ok_or_else(|| anyhow!("Description is required --description"))?,
            author: args
                .author
                .clone()
                .ok_or_else(|| anyhow!("Author is required --author"))?,
            license: args
                .license
                .clone()
                .ok_or_else(|| anyhow!("License is required --license"))?,
            project_type,
            package_behavior,
            language: args
                .language
                .clone()
                .ok_or_else(|| anyhow!("Language is required --language"))?,
            private: args.private,
            package_manager,
            git_repository_url,
            github_action: args.github_action,
            workspace: args.workspace,
        }
    } else {
        interactive_setup(&project_name, args)?
    };

    if config.workspace {
        create_workspace_project(&project_path, &config)?;
    } else {
        create_project(&project_path, &config)?;
    }

    // Run post init commands
    let codemod_path = if config.workspace {
        project_path
            .join("codemods")
            .join(get_codemod_dir_name(&config.name))
    } else {
        project_path.clone()
    };
    run_post_init_commands(&codemod_path, &config)?;

    let project_absolute_path = project_path.canonicalize()?;

    print_next_steps(&project_absolute_path, &config)?;

    Ok(())
}

/// Extracts the directory name from a codemod name (removes scope if present)
fn get_codemod_dir_name(name: &str) -> String {
    if let Some(pos) = name.find('/') {
        name[pos + 1..].to_string()
    } else {
        name.to_string()
    }
}

fn codemod_scope(name: &str) -> Option<&str> {
    let trimmed = name.trim();
    if !trimmed.starts_with('@') {
        return None;
    }

    let scope_end = trimmed.find('/')?;
    Some(&trimmed[1..scope_end])
}

fn workspace_root_readme_title(name: &str) -> String {
    codemod_scope(name)
        .map(|scope| format!("# @{} Codemods", scope))
        .unwrap_or_else(|| "# Organization Codemods".to_string())
}

fn workspace_root_readme_org_label(name: &str) -> String {
    codemod_scope(name)
        .map(|scope| format!("the `@{scope}` organization scope"))
        .unwrap_or_else(|| "your organization".to_string())
}

fn workspace_root_readme_scope_guidance(name: &str) -> String {
    codemod_scope(name)
        .map(|scope| {
            format!(
                "Publish packages under the `@{scope}/*` scope so they stay grouped in the Codemod Registry."
            )
        })
        .unwrap_or_else(|| {
            "Reserve an organization scope in Codemod before publishing so your packages stay grouped in the Codemod Registry.".to_string()
        })
}

fn normalize_project_type(selected: ProjectType) -> ProjectType {
    match selected {
        ProjectType::Shell | ProjectType::AstGrepYaml => {
            println!(
                "{} Deprecated project type selected; scaffolding a Hybrid (Shell + YAML + jssg) package",
                style("ℹ").cyan(),
            );
            ProjectType::Hybrid
        }
        other => other,
    }
}

fn package_behavior_from_flags(skill: bool, with_skill: bool) -> Result<PackageBehavior> {
    match (skill, with_skill) {
        (true, true) => Err(anyhow!("--skill and --with-skill cannot be used together")),
        (true, false) => Ok(PackageBehavior::SkillOnly),
        (false, true) => Ok(PackageBehavior::WorkflowAndSkill),
        (false, false) => Ok(PackageBehavior::WorkflowOnly),
    }
}

fn interactive_setup(project_name: &str, args: &Command) -> Result<ProjectConfig> {
    println!(
        "{} {}",
        ROCKET,
        style("Creating a new codemod project").bold()
    );
    println!();

    let (project_type, package_behavior) = if args.skill || args.with_skill {
        let package_behavior = package_behavior_from_flags(args.skill, args.with_skill)?;
        if package_behavior == PackageBehavior::SkillOnly && args.project_type.is_some() {
            return Err(anyhow!(
                "--project-type cannot be used with --skill. Remove --project-type for skill-only scaffolding."
            ));
        }
        let project_type = if package_behavior.includes_workflow() {
            if let Some(pt) = &args.project_type {
                normalize_project_type(pt.clone())
            } else {
                select_project_type()?
            }
        } else {
            ProjectType::AstGrepJs
        };
        (project_type, package_behavior)
    } else if let Some(pt) = &args.project_type {
        let project_type = normalize_project_type(pt.clone());
        let with_skill = Confirm::new("Would you like to add an agent skill?")
            .with_default(false)
            .prompt()?;
        let package_behavior = if with_skill {
            PackageBehavior::WorkflowAndSkill
        } else {
            PackageBehavior::WorkflowOnly
        };
        (project_type, package_behavior)
    } else {
        match select_interactive_codemod_type()? {
            InteractiveCodemodType::AgentSkill => {
                (ProjectType::AstGrepJs, PackageBehavior::SkillOnly)
            }
            InteractiveCodemodType::Jssg => {
                let with_skill = Confirm::new("Would you like to add an agent skill?")
                    .with_default(false)
                    .prompt()?;
                let package_behavior = if with_skill {
                    PackageBehavior::WorkflowAndSkill
                } else {
                    PackageBehavior::WorkflowOnly
                };
                (ProjectType::AstGrepJs, package_behavior)
            }
            InteractiveCodemodType::MultiStepWorkflow => {
                let with_skill = Confirm::new("Would you like to add an agent skill?")
                    .with_default(false)
                    .prompt()?;
                let package_behavior = if with_skill {
                    PackageBehavior::WorkflowAndSkill
                } else {
                    PackageBehavior::WorkflowOnly
                };
                (ProjectType::Hybrid, package_behavior)
            }
        }
    };

    // Language selection
    let language = if let Some(lang) = &args.language {
        lang.clone()
    } else {
        select_language()?
    };

    // Project details
    let name = if args.name.is_some() {
        args.name.clone().unwrap()
    } else {
        Text::new("Codemod name:")
            .with_help_message(
                "You can use the @scope/name format to create a scoped codemod. The scope is recommended to be your GitHub organization name."
            )
            .with_default(project_name)
            .prompt()?
    };

    let git_repository_url = if let Some(url) = &args.git_repository_url {
        url.clone()
    } else {
        Text::new("Git repository URL:")
            .with_validator(|input: &str| {
                if input.is_empty() || input.starts_with("https://") {
                    Ok(inquire::validator::Validation::Valid)
                } else {
                    Ok(inquire::validator::Validation::Invalid(
                        "Please enter a valid Git URL (must start with 'https://') or leave empty to skip."
                            .into(),
                    ))
                }
            })
            .prompt()?
    };

    let description = if let Some(desc) = &args.description {
        desc.clone()
    } else {
        Text::new("Description:")
            .with_default("Transform legacy code patterns")
            .prompt()?
    };

    let author = if let Some(auth) = &args.author {
        auth.clone()
    } else {
        Text::new("Author:")
            .with_default("Author <author@example.com>")
            .prompt()?
    };

    let license = if let Some(lic) = &args.license {
        lic.clone()
    } else {
        Text::new("License:").with_default("MIT").prompt()?
    };

    let private = if args.private {
        true
    } else {
        Confirm::new("Private package?")
            .with_default(false)
            .prompt()?
    };

    let workspace = if args.workspace {
        true
    } else {
        Confirm::new("Create a monorepo workspace structure?")
            .with_default(false)
            .with_help_message(
                "Organizes codemods in a 'codemods/' folder with shared workspace config",
            )
            .prompt()?
    };

    let requires_package_manager = (package_behavior.includes_workflow()
        && matches!(project_type, ProjectType::AstGrepJs | ProjectType::Hybrid))
        || workspace;
    let package_manager = if args.package_manager.is_some() {
        args.package_manager.clone()
    } else if requires_package_manager {
        Some(
            Select::new(
                "Which package manager would you like to use?",
                vec!["npm", "pnpm", "bun", "yarn"],
            )
            .prompt()?
            .to_string(),
        )
    } else {
        None
    };

    let github_action = if args.github_action {
        true
    } else {
        let help_msg = if workspace {
            "This creates .github/workflows/publish.yml triggered by <codemod-name>@v* tags"
        } else {
            "This creates .github/workflows/publish.yml using codemod/publish-action"
        };
        Confirm::new("Create GitHub Actions workflow for publishing?")
            .with_default(false)
            .with_help_message(help_msg)
            .prompt()?
    };

    Ok(ProjectConfig {
        name,
        description,
        author,
        license,
        project_type,
        package_behavior,
        language,
        private,
        package_manager,
        git_repository_url: if git_repository_url.is_empty() {
            None
        } else {
            Some(git_repository_url)
        },
        github_action,
        workspace,
    })
}

fn select_interactive_codemod_type() -> Result<InteractiveCodemodType> {
    let options = vec![
        "jssg codemod (covers most use cases)",
        "multi-step workflow (shell command, YAML & jssg)",
        "agent skill codemod",
    ];

    let selection =
        Select::new("What type of codemod would you like to create?", options).prompt()?;

    match selection {
        "jssg codemod (covers most use cases)" => Ok(InteractiveCodemodType::Jssg),
        "multi-step workflow (shell command, YAML & jssg)" => {
            Ok(InteractiveCodemodType::MultiStepWorkflow)
        }
        "agent skill codemod" => Ok(InteractiveCodemodType::AgentSkill),
        _ => Ok(InteractiveCodemodType::Jssg), // Default fallback
    }
}

fn select_project_type() -> Result<ProjectType> {
    let options = vec![
        "jssg codemod (covers most use cases)",
        "multi-step workflow (shell command, YAML & jssg)",
    ];

    let selection =
        Select::new("What type of codemod would you like to create?", options).prompt()?;

    match selection {
        "jssg codemod (covers most use cases)" => Ok(ProjectType::AstGrepJs),
        "multi-step workflow (shell command, YAML & jssg)" => Ok(ProjectType::Hybrid),
        _ => Ok(ProjectType::AstGrepJs), // Default fallback
    }
}

fn select_language() -> Result<String> {
    let options = vec![
        "JavaScript/TypeScript",
        "Python",
        "Rust",
        "Go",
        "Java",
        "HTML",
        "XML",
        "CSS",
        "Kotlin",
        "C#",
        "C++",
        "C",
        "PHP",
        "Ruby",
        "Elixir",
        "Json",
        "Yaml",
        "Toml",
        "Other",
    ];

    let selection = Select::new("Which language would you like to target?", options).prompt()?;

    let language = match selection {
        "JavaScript/TypeScript" => "typescript",
        "Python" => "python",
        "Rust" => "rust",
        "Go" => "go",
        "Java" => "java",
        "HTML" => "html",
        "XML" => "xml",
        "CSS" => "css",
        "Kotlin" => "kotlin",
        "C#" => "csharp",
        "C++" => "cpp",
        "C" => "c",
        "PHP" => "php",
        "Ruby" => "ruby",
        "Elixir" => "elixir",
        "Json" => "json",
        "Yaml" => "yaml",
        "Toml" => "toml",
        "Other" => {
            let custom = Text::new("Enter language name:").prompt()?;
            return Ok(custom);
        }
        _ => "typescript",
    };

    Ok(language.to_string())
}

fn create_project(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    // Create project directory
    fs::create_dir_all(project_path)?;

    // Create codemod.yaml
    create_manifest(project_path, config)?;

    // Always create workflow.ts (workflow-first package model)
    create_workflow(project_path, config)?;

    // Create workflow project structure
    if config.package_behavior.includes_workflow() {
        match config.project_type {
            ProjectType::Shell => create_shell_project(project_path, config)?,
            ProjectType::AstGrepJs => create_js_astgrep_project(project_path, config)?,
            ProjectType::AstGrepYaml => create_astgrep_yaml_project(project_path, config)?,
            ProjectType::Hybrid => create_hybrid_project(project_path, config)?,
        }
    }

    // Create skill assets if requested
    if config.package_behavior.includes_skill() {
        create_skill_project(project_path, config)?;
    }

    // Create common files
    create_gitignore(project_path)?;
    create_readme(project_path, config)?;

    // Create GitHub Actions workflow if requested
    if config.github_action {
        create_github_action(project_path, false)?;
    }

    info!("✓ Created {} project", config.name);
    Ok(())
}

fn create_manifest(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    let repository_line = if let Some(url) = &config.git_repository_url {
        format!("repository: \"{}\"", url)
    } else {
        String::new()
    };

    let template = if config.package_behavior == PackageBehavior::SkillOnly {
        SKILL_CODEMOD_TEMPLATE
    } else {
        CODEMOD_TEMPLATE
    };

    let manifest_content = template
        .replace("{name}", &config.name)
        .replace("{description}", &config.description)
        .replace("{author}", &config.author)
        .replace("{license}", &config.license)
        .replace("{language}", &config.language)
        .replace(
            "{access}",
            if config.private { "private" } else { "public" },
        )
        .replace(
            "{visibility}",
            if config.private { "private" } else { "public" },
        )
        .replace("{repository}", &repository_line);

    fs::write(project_path.join("codemod.yaml"), manifest_content)?;
    Ok(())
}

fn create_workflow(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    let default_skill_path = expected_authored_skill_relative_file(&config.name);
    let install_skill = config.package_behavior == PackageBehavior::WorkflowAndSkill;
    let workflow = match (config.package_behavior, &config.project_type) {
        (PackageBehavior::SkillOnly, _) => workflow_typescript(
            &SKILL_WORKFLOW_TEMPLATE
                .replace("{name}", &config.name)
                .replace("{skill_path}", &default_skill_path),
        )?,
        (_, ProjectType::AstGrepJs | ProjectType::Hybrid) => {
            let mut before = Vec::new();
            let mut after = Vec::new();
            let mut imports = Vec::new();
            if config.project_type == ProjectType::Hybrid {
                imports.push("shell");
                before.push(
                    r#"await step("Setup", shell("$CODEMOD_PATH/scripts/setup.sh"));"#.to_string(),
                );
                before.push(
                    r#"await step("Transform", shell("$CODEMOD_PATH/scripts/transform.sh"));"#
                        .to_string(),
                );
                if !hybrid_without_rules(&config.language) {
                    imports.push("astGrep");
                    before.push(
                        r#"await step("Apply ast-grep rules", forEachFile(astGrep("rules/config.yml")));"#
                            .to_string(),
                    );
                }
                after.push(
                    r#"await step("Cleanup", shell("$CODEMOD_PATH/scripts/cleanup.sh"));"#
                        .to_string(),
                );
            }
            if install_skill {
                imports.push("installSkill");
                after.push(format!(
                    "await step(\"Install package skill\", installSkill({{ package: {}, path: {} }}));",
                    serde_json::to_string(&config.name)?,
                    serde_json::to_string(&default_skill_path)?,
                ));
            }
            with_steps(
                callback_workflow_template(&config.language),
                &imports,
                &before,
                &after,
            )?
        }
        (_, project_type) => {
            let template = match project_type {
                ProjectType::Shell => SHELL_WORKFLOW_TEMPLATE,
                _ => ASTGREP_YAML_WORKFLOW_TEMPLATE,
            };
            let mut yaml = template.replace("{language}", &config.language);
            if install_skill {
                yaml.push_str(
                    &INSTALL_SKILL_NODE_TEMPLATE
                        .replace("{name}", &config.name)
                        .replace("{skill_path}", &default_skill_path),
                );
            }
            workflow_typescript(&yaml)?
        }
    };

    fs::write(project_path.join("workflow.ts"), workflow)?;
    Ok(())
}

/// Languages whose hybrid packages have no ast-grep rule step: ast-grep's
/// YAML rules do not support them.
fn hybrid_without_rules(language: &str) -> bool {
    language == "toml"
}

/// A callback workflow template with more steps before and after its own,
/// and the `codemod/workflow` imports they use.
fn with_steps(
    template: &str,
    imports: &[&str],
    before: &[String],
    after: &[String],
) -> Result<String> {
    const MAIN: &str = "export default async function main() {\n";
    const IMPORT: &str = " } from \"codemod/workflow\";\n";
    let main = template
        .find(MAIN)
        .context("workflow template has no main function")?
        + MAIN.len();
    let end = template
        .rfind("}\n")
        .filter(|&end| end >= main)
        .context("workflow template does not end with main")?;
    let import_end = template
        .find(IMPORT)
        .context("workflow template does not import codemod/workflow")?;
    let import_start = template[..import_end]
        .rfind("import { ")
        .context("workflow template does not import codemod/workflow")?
        + "import { ".len();

    let mut names: Vec<&str> = template[import_start..import_end].split(", ").collect();
    names.extend_from_slice(imports);
    names.sort_unstable();
    names.dedup();
    let lines =
        |steps: &[String]| -> String { steps.iter().map(|step| format!("  {step}\n")).collect() };
    Ok(format!(
        "{}{}{}{}{}{}{}",
        &template[..import_start],
        names.join(", "),
        &template[import_end..main],
        lines(before),
        &template[main..end],
        lines(after),
        &template[end..],
    ))
}

/// The shell, ast-grep and skill templates are written as workflow.yaml;
/// new packages get the equivalent workflow.ts.
fn workflow_typescript(yaml: &str) -> Result<String> {
    let workflow: butterflow_models::Workflow =
        serde_yaml::from_str(yaml).context("invalid workflow template")?;
    let translated = codemod_workflow::compat::translate(&workflow);
    Ok(translated
        .source
        .lines()
        .skip_while(|line| line.starts_with("// "))
        .collect::<Vec<_>>()
        .join("\n")
        .trim_start()
        .to_string()
        + "\n")
}

fn create_shell_project(project_path: &Path, _config: &ProjectConfig) -> Result<()> {
    // Create scripts directory
    let scripts_dir = project_path.join("scripts");
    fs::create_dir_all(&scripts_dir)?;

    // Create setup script
    fs::write(scripts_dir.join("setup.sh"), SHELL_SETUP_SCRIPT)?;

    // Create transform script
    fs::write(scripts_dir.join("transform.sh"), SHELL_TRANSFORM_SCRIPT)?;

    // Create cleanup script
    fs::write(scripts_dir.join("cleanup.sh"), SHELL_CLEANUP_SCRIPT)?;

    Ok(())
}

/// package.json and tsconfig.json for a package with TypeScript in it (the
/// workflow.ts and JSSG scripts). The package depends on `codemod`: the CLI
/// its scripts run, which also ships the types.
fn create_typescript_package_files(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    let package_json = JS_PACKAGE_JSON_TEMPLATE
        .replace("{name}", &config.name)
        .replace("{description}", &config.description)
        .replace("{package_manager}", selected_package_manager_spec(config))
        .replace("{cli_version}", crate::CLI_VERSION);
    fs::write(project_path.join("package.json"), package_json)?;
    fs::write(project_path.join("tsconfig.json"), JS_TSCONFIG_TEMPLATE)?;
    Ok(())
}

fn create_js_astgrep_project(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    // The transform is the forEachFile callback in workflow.ts.
    create_typescript_package_files(project_path, config)?;
    create_js_tests(project_path, config)?;
    Ok(())
}

fn create_astgrep_yaml_project(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    let rules_dir = project_path.join("rules");
    fs::create_dir_all(&rules_dir)?;

    let config_file = match config.language.as_str() {
        "javascript" | "typescript" => ASTGREP_PATTERNS_FOR_JAVASCRIPT,
        "python" => ASTGREP_PATTERNS_FOR_PYTHON,
        "rust" => ASTGREP_PATTERNS_FOR_RUST,
        "go" => ASTGREP_PATTERNS_FOR_GO,
        "java" => ASTGREP_PATTERNS_FOR_JAVA,
        "html" => ASTGREP_PATTERNS_FOR_HTML,
        "xml" => ASTGREP_PATTERNS_FOR_XML,
        "css" => ASTGREP_PATTERNS_FOR_CSS,
        "kotlin" => ASTGREP_PATTERNS_FOR_KOTLIN,
        "csharp" => ASTGREP_PATTERNS_FOR_CSHARP,
        "cpp" => ASTGREP_PATTERNS_FOR_CPP,
        "c" => ASTGREP_PATTERNS_FOR_C,
        "php" => ASTGREP_PATTERNS_FOR_PHP,
        "ruby" => ASTGREP_PATTERNS_FOR_RUBY,
        "elixir" => ASTGREP_PATTERNS_FOR_ELIXIR,
        "json" => ASTGREP_PATTERNS_FOR_JSON,
        "yaml" => ASTGREP_PATTERNS_FOR_YAML,
        _ => ASTGREP_PATTERNS_FOR_JAVASCRIPT,
    };
    fs::write(rules_dir.join("config.yml"), config_file)?;

    // Create tests directory
    let tests_dir = project_path.join("tests");
    fs::create_dir_all(tests_dir.join("input"))?;
    fs::create_dir_all(tests_dir.join("expected"))?;

    Ok(())
}

fn create_hybrid_project(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    // Create scripts directory
    let scripts_dir = project_path.join("scripts");
    fs::create_dir_all(&scripts_dir)?;

    // Copy shell scripts
    fs::write(scripts_dir.join("setup.sh"), SHELL_SETUP_SCRIPT)?;
    fs::write(scripts_dir.join("transform.sh"), SHELL_TRANSFORM_SCRIPT)?;
    fs::write(scripts_dir.join("cleanup.sh"), SHELL_CLEANUP_SCRIPT)?;

    // ast-grep rule files cannot target TOML.
    if !hybrid_without_rules(&config.language) {
        let rules_dir = project_path.join("rules");
        fs::create_dir_all(&rules_dir)?;

        let config_file = match config.language.as_str() {
            "javascript" | "typescript" => ASTGREP_PATTERNS_FOR_JAVASCRIPT,
            "python" => ASTGREP_PATTERNS_FOR_PYTHON,
            "rust" => ASTGREP_PATTERNS_FOR_RUST,
            "go" => ASTGREP_PATTERNS_FOR_GO,
            "java" => ASTGREP_PATTERNS_FOR_JAVA,
            "html" => ASTGREP_PATTERNS_FOR_HTML,
            "xml" => ASTGREP_PATTERNS_FOR_XML,
            "css" => ASTGREP_PATTERNS_FOR_CSS,
            "kotlin" => ASTGREP_PATTERNS_FOR_KOTLIN,
            "csharp" => ASTGREP_PATTERNS_FOR_CSHARP,
            "cpp" => ASTGREP_PATTERNS_FOR_CPP,
            "c" => ASTGREP_PATTERNS_FOR_C,
            "php" => ASTGREP_PATTERNS_FOR_PHP,
            "ruby" => ASTGREP_PATTERNS_FOR_RUBY,
            "elixir" => ASTGREP_PATTERNS_FOR_ELIXIR,
            "json" => ASTGREP_PATTERNS_FOR_JSON,
            "yaml" => ASTGREP_PATTERNS_FOR_YAML,
            _ => ASTGREP_PATTERNS_FOR_JAVASCRIPT,
        };
        fs::write(rules_dir.join("config.yml"), config_file)?;
    }

    create_js_tests(project_path, config)?;
    create_typescript_package_files(project_path, config)?;

    Ok(())
}

fn create_skill_project(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    let skill_root = project_path
        .join(AGENTS_SKILL_ROOT_RELATIVE_PATH)
        .join(get_codemod_dir_name(&config.name));
    let references_dir = skill_root.join("references");
    fs::create_dir_all(&references_dir)?;

    let skill_content = SKILL_TEMPLATE
        .replace("{name}", &config.name)
        .replace("{description}", &config.description);
    fs::write(skill_root.join("SKILL.md"), skill_content)?;

    let references_index = SKILL_REFERENCES_INDEX_TEMPLATE.replace("{name}", &config.name);
    fs::write(references_dir.join("index.md"), references_index)?;

    let references_usage = SKILL_REFERENCES_USAGE_TEMPLATE
        .replace("{name}", &config.name)
        .replace("{description}", &config.description);
    fs::write(references_dir.join("usage.md"), references_usage)?;

    Ok(())
}

/// The starter fixture for a language: its file extension, input, and the
/// output a JSSG package's workflow leaves.
fn js_fixture(language: &str) -> Option<(&'static str, &'static str, &'static str)> {
    // The extension must match the files the workflow selects
    // (`templates/js-astgrep/workflows/*.ts`), or it never picks it up.
    Some(match language {
        "javascript" => ("js", JS_TEST_INPUT, JS_TEST_EXPECTED),
        "typescript" => ("ts", JS_TEST_INPUT, JS_TEST_EXPECTED),
        "python" => ("py", PYTHON_TEST_INPUT, PYTHON_TEST_EXPECTED),
        "rust" => ("rs", RUST_TEST_INPUT, RUST_TEST_EXPECTED),
        "go" => ("go", GO_TEST_INPUT, GO_TEST_EXPECTED),
        "java" => ("java", JAVA_TEST_INPUT, JAVA_TEST_EXPECTED),
        "xml" => ("xml", XML_TEST_INPUT, XML_TEST_EXPECTED),
        "csharp" => ("cs", CSHARP_TEST_INPUT, CSHARP_TEST_EXPECTED),
        "cpp" => ("cpp", CPP_TEST_INPUT, CPP_TEST_EXPECTED),
        "c" => ("c", C_TEST_INPUT, C_TEST_EXPECTED),
        "php" => ("php", PHP_TEST_INPUT, PHP_TEST_EXPECTED),
        "ruby" => ("rb", RUBY_TEST_INPUT, RUBY_TEST_EXPECTED),
        "elixir" => ("ex", ELIXIR_TEST_INPUT, ELIXIR_TEST_EXPECTED),
        "html" => ("html", HTML_TEST_INPUT, HTML_TEST_EXPECTED),
        "css" => ("css", CSS_TEST_INPUT, CSS_TEST_EXPECTED),
        "kotlin" => ("kt", KOTLIN_TEST_INPUT, KOTLIN_TEST_EXPECTED),
        "json" => ("json", JSON_TEST_INPUT, JSON_TEST_EXPECTED),
        "yaml" => ("yaml", YAML_TEST_INPUT, YAML_TEST_EXPECTED),
        "toml" => ("toml", TOML_TEST_INPUT, TOML_TEST_EXPECTED),
        _ => return None,
    })
}

/// What a hybrid package's whole workflow leaves of the starter fixture: its
/// shell steps and ast-grep rules run before the JSSG transform.
fn hybrid_expected(language: &str) -> Option<&'static str> {
    // Only where the rules change the outcome; elsewhere it is the JSSG
    // package's.
    Some(match language {
        "typescript" => include_str!("../templates/hybrid/tests/fixtures/expected.ts"),
        "rust" => include_str!("../templates/hybrid/tests/fixtures/expected.rs"),
        "go" => include_str!("../templates/hybrid/tests/fixtures/expected.go"),
        "java" => include_str!("../templates/hybrid/tests/fixtures/expected.java"),
        "html" => include_str!("../templates/hybrid/tests/fixtures/expected.html"),
        "css" => include_str!("../templates/hybrid/tests/fixtures/expected.css"),
        "c" => include_str!("../templates/hybrid/tests/fixtures/expected.c"),
        "yaml" => include_str!("../templates/hybrid/tests/fixtures/expected.yaml"),
        _ => return None,
    })
}

fn create_js_tests(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    let fixtures = project_path.join("tests").join("fixtures");
    fs::create_dir_all(&fixtures)?;
    let Some((ext, input, expected)) = js_fixture(&config.language) else {
        return Ok(());
    };
    let expected = match config.project_type {
        ProjectType::Hybrid => hybrid_expected(&config.language).unwrap_or(expected),
        _ => expected,
    };
    fs::write(fixtures.join(format!("input.{ext}")), input)?;
    fs::write(fixtures.join(format!("expected.{ext}")), expected)?;
    Ok(())
}

fn create_gitignore(project_path: &Path) -> Result<()> {
    fs::write(project_path.join(".gitignore"), GITIGNORE_TEMPLATE)?;
    Ok(())
}

fn create_readme(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    let package_manager = selected_package_manager(config);
    let test_command = if config.package_behavior == PackageBehavior::SkillOnly {
        format!(
            "{} {}",
            codemod_cli_command_for_package_manager(package_manager),
            config.name
        )
    } else {
        match config.project_type {
            ProjectType::Shell => "bash scripts/transform.sh".to_string(),
            ProjectType::AstGrepJs => package_manager_test_command(package_manager),
            ProjectType::AstGrepYaml => "ast-grep test rules/".to_string(),
            ProjectType::Hybrid => package_manager_test_command(package_manager),
        }
    };

    let template = if config.package_behavior == PackageBehavior::SkillOnly {
        SKILL_README_TEMPLATE
    } else {
        README_TEMPLATE
    };

    let mut readme_content = template
        .replace("{name}", &config.name)
        .replace("{description}", &config.description)
        .replace("{language}", &config.language)
        .replace("{test_command}", &test_command)
        .replace("{license}", &config.license);

    if config.package_behavior == PackageBehavior::WorkflowAndSkill {
        readme_content.push_str(&format!(
            r#"
## Skill Installation

```bash
{} {}
```
"#,
            codemod_cli_command_for_package_manager(package_manager),
            config.name
        ));
    }

    fs::write(project_path.join("README.md"), readme_content)?;
    Ok(())
}

fn create_github_action(project_path: &Path, workspace: bool) -> Result<()> {
    let workflows_dir = project_path.join(".github").join("workflows");
    fs::create_dir_all(&workflows_dir)?;
    let template = if workspace {
        GITHUB_ACTION_WORKSPACE_TEMPLATE
    } else {
        GITHUB_ACTION_TEMPLATE
    };
    fs::write(workflows_dir.join("publish.yml"), template)?;
    Ok(())
}

fn create_workspace_root_readme(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    let readme_content = WORKSPACE_SKILL_ROOT_README_TEMPLATE
        .replace("{title}", &workspace_root_readme_title(&config.name))
        .replace(
            "{org_label}",
            &workspace_root_readme_org_label(&config.name),
        )
        .replace(
            "{scope_guidance}",
            &workspace_root_readme_scope_guidance(&config.name),
        );

    fs::write(project_path.join("README.md"), readme_content)?;
    Ok(())
}

fn create_workspace_project(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    // Create root workspace directory
    fs::create_dir_all(project_path)?;

    // Create codemods directory
    let codemods_dir = project_path.join("codemods");
    fs::create_dir_all(&codemods_dir)?;

    // Get the codemod directory name (without scope)
    let codemod_dir_name = get_codemod_dir_name(&config.name);
    let codemod_path = codemods_dir.join(&codemod_dir_name);

    // Create the codemod project inside codemods/<name>/
    create_codemod_in_workspace(&codemod_path, config)?;

    // Create root workspace files
    create_workspace_root_package_json(project_path, config)?;
    create_gitignore(project_path)?;
    if config.package_behavior.includes_skill() {
        create_workspace_root_readme(project_path, config)?;
    }

    // Create GitHub Actions workflow at root level if requested
    if config.github_action {
        create_github_action(project_path, true)?;
    }

    info!("✓ Created {} workspace project", config.name);
    Ok(())
}

fn create_codemod_in_workspace(codemod_path: &Path, config: &ProjectConfig) -> Result<()> {
    // Create codemod directory
    fs::create_dir_all(codemod_path)?;

    // Create codemod.yaml
    create_manifest(codemod_path, config)?;

    // Always create workflow.ts (workflow-first package model)
    create_workflow(codemod_path, config)?;

    // Create workflow project structure
    if config.package_behavior.includes_workflow() {
        match config.project_type {
            ProjectType::Shell => create_shell_project(codemod_path, config)?,
            ProjectType::AstGrepJs => create_js_astgrep_project(codemod_path, config)?,
            ProjectType::AstGrepYaml => create_astgrep_yaml_project(codemod_path, config)?,
            ProjectType::Hybrid => create_hybrid_project(codemod_path, config)?,
        }
    }

    // Create skill assets if requested
    if config.package_behavior.includes_skill() {
        create_skill_project(codemod_path, config)?;
    }

    // Create codemod-specific readme
    create_readme(codemod_path, config)?;

    Ok(())
}

fn create_workspace_root_package_json(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    let package_manager = config.package_manager.clone().unwrap_or("npm".to_string());

    let workspaces_config = match package_manager.as_str() {
        "pnpm" => {
            // pnpm uses pnpm-workspace.yaml
            let pnpm_workspace = "packages:\n  - \"codemods/*\"\n";
            fs::write(project_path.join("pnpm-workspace.yaml"), pnpm_workspace)?;
            "" // No workspaces field in package.json for pnpm
        }
        _ => {
            r#",
  "workspaces": [
    "codemods/*"
  ]"#
        }
    };

    let root_package_json = format!(
        r#"{{
  "name": "{}-workspace",
  "private": true,
  "description": "Monorepo workspace for codemods"{}
}}"#,
        get_codemod_dir_name(&config.name),
        workspaces_config
    );

    fs::write(project_path.join("package.json"), root_package_json)?;
    Ok(())
}

fn run_post_init_commands(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    if !config.package_behavior.includes_workflow() {
        return Ok(());
    }

    match config.project_type {
        ProjectType::AstGrepJs | ProjectType::Hybrid => {
            let package_manager = selected_package_manager(config);
            let install_command = package_manager_install_command(package_manager);

            let output = ProcessCommand::new(package_manager)
                .arg("install")
                .current_dir(project_path)
                .output();

            println!("{} Installing dependencies...", style("⏳").yellow());

            match output {
                Ok(result) => {
                    if result.status.success() {
                        println!("{CHECKMARK} Dependencies installed successfully");
                    } else {
                        let stderr = String::from_utf8_lossy(&result.stderr);
                        println!(
                            "{} Failed to install dependencies: {}",
                            style("⚠").red(),
                            stderr
                        );
                        println!(
                            "  You can run {} manually later",
                            style(&install_command).cyan()
                        );
                    }
                }
                Err(e) => {
                    println!("{} {} not found: {}", style("⚠").red(), package_manager, e);
                    println!(
                        "  You can run {} manually later",
                        style(&install_command).cyan()
                    );
                }
            }

            // For hybrid projects, also make shell scripts executable
            if config.project_type == ProjectType::Hybrid {
                println!("{} Making scripts executable...", style("⏳").yellow());

                let scripts_dir = project_path.join("scripts");
                if let Ok(entries) = fs::read_dir(&scripts_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().and_then(|s| s.to_str()) == Some("sh") {
                            #[cfg(unix)]
                            {
                                use std::os::unix::fs::PermissionsExt;
                                if let Ok(mut perms) = fs::metadata(&path).map(|m| m.permissions())
                                {
                                    perms.set_mode(0o755);
                                    if fs::set_permissions(&path, perms).is_ok() {
                                        println!(
                                            "{} Made {} executable",
                                            CHECKMARK,
                                            path.file_name().unwrap().to_string_lossy()
                                        );
                                    }
                                }
                            }
                            #[cfg(not(unix))]
                            {
                                println!(
                                    "{} {} (executable permission not set on non-Unix systems)",
                                    CHECKMARK,
                                    path.file_name().unwrap().to_string_lossy()
                                );
                            }
                        }
                    }
                }
            }
        }
        ProjectType::Shell => {
            println!("{} Making scripts executable...", style("⏳").yellow());

            let scripts_dir = project_path.join("scripts");
            if let Ok(entries) = fs::read_dir(&scripts_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|s| s.to_str()) == Some("sh") {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            if let Ok(mut perms) = fs::metadata(&path).map(|m| m.permissions()) {
                                perms.set_mode(0o755);
                                if fs::set_permissions(&path, perms).is_ok() {
                                    println!(
                                        "{} Made {} executable",
                                        CHECKMARK,
                                        path.file_name().unwrap().to_string_lossy()
                                    );
                                }
                            }
                        }
                        #[cfg(not(unix))]
                        {
                            println!(
                                "{} {} (executable permission not set on non-Unix systems)",
                                CHECKMARK,
                                path.file_name().unwrap().to_string_lossy()
                            );
                        }
                    }
                }
            }
        }
        ProjectType::AstGrepYaml => {
            // No post-init commands needed for YAML projects
        }
    }

    Ok(())
}

fn print_next_steps(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    let codemod_dir_name = get_codemod_dir_name(&config.name);
    let package_manager = selected_package_manager(config);
    let codemod_command = if installs_codemod(config) {
        installed_codemod_command(package_manager)
    } else {
        codemod_cli_command_for_package_manager(package_manager)
    };

    println!();
    if config.workspace {
        println!(
            "{} Created {} workspace",
            CHECKMARK,
            style(&config.name).green().bold()
        );
        println!("{CHECKMARK} Created workspace structure with codemods/ folder");
        println!(
            "{CHECKMARK} Created initial codemod in codemods/{}/",
            codemod_dir_name
        );
    } else {
        println!(
            "{} Created {} project",
            CHECKMARK,
            style(&config.name).green().bold()
        );
        println!("{CHECKMARK} Generated codemod.yaml manifest");
        if config.package_behavior.includes_workflow() {
            println!("{CHECKMARK} Generated workflow.ts definition");
        }
        if config.package_behavior.includes_skill() {
            println!(
                "{CHECKMARK} Generated skill assets under {}/{}/",
                AGENTS_SKILL_ROOT_RELATIVE_PATH, codemod_dir_name
            );
        }
        println!("{CHECKMARK} Created project structure");
    }
    if config.github_action {
        if config.workspace {
            println!(
                "{CHECKMARK} Created GitHub Actions workflow (triggers on {}@v* tags)",
                codemod_dir_name
            );
        } else {
            println!("{CHECKMARK} Created GitHub Actions workflow (.github/workflows/publish.yml)");
        }
    }
    println!();
    println!("{}", style("Next steps:").bold());

    if config.package_behavior == PackageBehavior::SkillOnly {
        println!();
        println!(
            "  {}",
            style("Run the package to install skill behavior")
                .bold()
                .cyan()
        );
        println!(
            "  {}",
            style(format!("{} {}", codemod_command, config.name)).dim()
        );
        println!();
        println!(
            "  {}",
            style("Run with harness after install").bold().cyan()
        );
        println!(
            "  {}",
            style("Use your harness to execute the installed skill instructions.").dim()
        );
    } else {
        // Determine the path to the workflow.ts
        let workflow_path = if config.workspace {
            format!(
                "{}/codemods/{}/workflow.ts",
                project_path.display(),
                codemod_dir_name
            )
        } else {
            format!("{}/workflow.ts", project_path.display())
        };

        println!();
        println!("  {}", style("Validate your workflow").bold().cyan());
        println!(
            "  {}",
            style(format!(
                "{} workflow validate -w {}",
                codemod_command, workflow_path
            ))
            .dim()
        );
        println!();
        println!("  {}", style("Run your codemod locally").bold().cyan());
        println!(
            "  {}",
            style("Warning: Target path is where you are and please run it on git tracked path")
                .yellow()
                .bold()
        );
        println!(
            "  {}",
            style(format!(
                "{} workflow run -w {} --target ./some/target/path",
                codemod_command, workflow_path
            ))
            .dim()
        );

        if config.package_behavior.includes_skill() {
            println!();
            println!(
                "  {}",
                style("Run package and accept skill-install prompt")
                    .bold()
                    .cyan()
            );
            println!(
                "  {}",
                style(format!("{} {}", codemod_command, config.name)).dim()
            );
        }
    }
    if config.github_action {
        println!();
        println!(
            "  {}",
            style("Set up trusted publisher for GitHub Actions")
                .bold()
                .cyan()
        );
        println!(
            "  {}",
            style("Configure your repository at codemod.com to enable OIDC publishing:").dim()
        );
        println!(
            "  {}",
            style("https://go.codemod.com/trusted-publishers")
                .underlined()
                .dim()
        );
        if config.workspace {
            println!();
            println!(
                "  {}",
                style("To publish a codemod, create a tag like:").dim()
            );
            println!(
                "  {}",
                style(format!(
                    "git tag {}@v1.0.0 && git push origin {}@v1.0.0",
                    codemod_dir_name, codemod_dir_name
                ))
                .cyan()
            );
        }
    }

    println!();
    println!(
        "  {}",
        style("👉 Check out the docs to learn how to publish your codemod!")
            .bold()
            .cyan()
    );
    println!(
        "  {}",
        style("https://go.codemod.com/docs").underlined().dim()
    );

    Ok(())
}

fn selected_package_manager(config: &ProjectConfig) -> &str {
    config.package_manager.as_deref().unwrap_or("npm")
}

fn selected_package_manager_spec(config: &ProjectConfig) -> &'static str {
    match selected_package_manager(config) {
        "yarn" => "yarn@4.x",
        "pnpm" => "pnpm@10.x",
        "bun" => "bun@1.x",
        _ => "npm@10.x",
    }
}

/// Whether the project gets a package.json that depends on `codemod`.
fn installs_codemod(config: &ProjectConfig) -> bool {
    config.package_behavior.includes_workflow()
        && matches!(
            config.project_type,
            ProjectType::AstGrepJs | ProjectType::Hybrid
        )
}

/// The project's own `codemod`, run through its package manager.
fn installed_codemod_command(package_manager: &str) -> &'static str {
    match package_manager {
        "yarn" => "yarn codemod",
        "pnpm" => "pnpm codemod",
        "bun" => "bunx codemod",
        _ => "npx codemod",
    }
}

/// `codemod` for someone who has not installed it, such as a user running a
/// published package.
fn codemod_cli_command_for_package_manager(package_manager: &str) -> &'static str {
    match package_manager {
        "npm" => "npx codemod@latest",
        "yarn" => "yarn dlx codemod@latest",
        "pnpm" => "pnpm dlx codemod@latest",
        "bun" => "bunx codemod@latest",
        _ => "npx codemod@latest",
    }
}

fn package_manager_install_command(package_manager: &str) -> String {
    format!("{package_manager} install")
}

fn package_manager_test_command(package_manager: &str) -> String {
    match package_manager {
        "yarn" => "yarn test".to_string(),
        "pnpm" => "pnpm test".to_string(),
        "bun" => "bun run test".to_string(),
        _ => "npm test".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::manifest::CodemodManifest;
    use crate::utils::package_validation::validate_skill_behavior;
    use tempfile::tempdir;

    fn skill_project_config(workspace: bool) -> ProjectConfig {
        ProjectConfig {
            name: "@codemod/sample-skill".to_string(),
            description: "Sample skill package".to_string(),
            author: "Codemod Team <team@codemod.com>".to_string(),
            license: "MIT".to_string(),
            project_type: ProjectType::AstGrepJs,
            package_behavior: PackageBehavior::SkillOnly,
            language: "typescript".to_string(),
            private: false,
            package_manager: if workspace {
                Some("npm".to_string())
            } else {
                None
            },
            git_repository_url: Some("https://github.com/codemod/sample-skill".to_string()),
            github_action: false,
            workspace,
        }
    }

    #[test]
    fn create_project_skill_only_generates_skill_files_with_install_workflow() {
        let temp_dir = tempdir().unwrap();
        let project_path = temp_dir.path().join("sample-skill");
        let config = skill_project_config(false);

        create_project(&project_path, &config).unwrap();

        let skill_root = project_path
            .join(AGENTS_SKILL_ROOT_RELATIVE_PATH)
            .join("sample-skill");
        assert!(project_path.join("codemod.yaml").is_file());
        assert!(skill_root.join("SKILL.md").is_file());
        assert!(skill_root.join("references/index.md").is_file());
        assert!(skill_root.join("references/usage.md").is_file());
        assert!(project_path.join("README.md").is_file());
        assert!(project_path.join("workflow.ts").is_file());

        let manifest = fs::read_to_string(project_path.join("codemod.yaml")).unwrap();
        assert!(manifest.contains("capabilities:"));
        assert!(manifest.contains("workflow: \"workflow.ts\""));
        let parsed_manifest: CodemodManifest = serde_yaml::from_str(&manifest).unwrap();
        let validation = validate_skill_behavior(&project_path, &parsed_manifest).unwrap();
        assert_eq!(validation.linked_reference_count, 1);
        let workflow = fs::read_to_string(project_path.join("workflow.ts")).unwrap();
        assert!(workflow.contains("installSkill({"));
        assert!(
            workflow.contains("package: \"@codemod/sample-skill\""),
            "{workflow}"
        );
        assert!(workflow.contains("path: \"./agents/skill/sample-skill/SKILL.md\""));

        let readme = fs::read_to_string(project_path.join("README.md")).unwrap();
        assert!(readme.contains("npx codemod@latest @codemod/sample-skill"));
    }

    #[test]
    fn create_workspace_skill_only_places_skill_package_in_codemods_folder() {
        let temp_dir = tempdir().unwrap();
        let workspace_path = temp_dir.path().join("workspace");
        let config = skill_project_config(true);

        create_workspace_project(&workspace_path, &config).unwrap();

        let codemod_path = workspace_path.join("codemods/sample-skill");
        let skill_root = codemod_path
            .join(AGENTS_SKILL_ROOT_RELATIVE_PATH)
            .join("sample-skill");
        let root_readme = fs::read_to_string(workspace_path.join("README.md")).unwrap();
        assert!(workspace_path.join("package.json").is_file());
        assert!(workspace_path.join(".gitignore").is_file());
        assert!(workspace_path.join("README.md").is_file());
        assert!(codemod_path.join("codemod.yaml").is_file());
        assert!(skill_root.join("SKILL.md").is_file());
        assert!(skill_root.join("references/index.md").is_file());
        assert!(codemod_path.join("workflow.ts").is_file());
        let manifest = fs::read_to_string(codemod_path.join("codemod.yaml")).unwrap();
        let parsed_manifest: CodemodManifest = serde_yaml::from_str(&manifest).unwrap();
        let validation = validate_skill_behavior(&codemod_path, &parsed_manifest).unwrap();
        assert_eq!(validation.linked_reference_count, 1);
        let workflow = fs::read_to_string(codemod_path.join("workflow.ts")).unwrap();
        assert!(workflow.contains("installSkill({"));
        assert!(workflow.contains("path: \"./agents/skill/sample-skill/SKILL.md\""));

        let readme = fs::read_to_string(codemod_path.join("README.md")).unwrap();
        assert!(readme.contains("npx codemod@latest @codemod/sample-skill"));
        assert!(root_readme.contains("## One-time setup"));
        assert!(root_readme.contains("codemods/<slug>/"));
        assert!(root_readme.contains("`@codemod/*`"));
        assert!(root_readme.contains("## Running codemods"));
    }

    #[test]
    fn create_manifest_for_workflow_projects_has_required_workflow_fields() {
        let temp_dir = tempdir().unwrap();
        let project_path = temp_dir.path().join("workflow-project");
        fs::create_dir_all(&project_path).unwrap();

        let config = ProjectConfig {
            name: "workflow-project".to_string(),
            description: "Workflow package".to_string(),
            author: "Codemod Team <team@codemod.com>".to_string(),
            license: "MIT".to_string(),
            project_type: ProjectType::Hybrid,
            package_behavior: PackageBehavior::WorkflowOnly,
            language: "typescript".to_string(),
            private: false,
            package_manager: Some("pnpm".to_string()),
            git_repository_url: None,
            github_action: false,
            workspace: false,
        };

        create_manifest(&project_path, &config).unwrap();
        let manifest = fs::read_to_string(project_path.join("codemod.yaml")).unwrap();

        // The init template now scaffolds the multi-workflow `workflows:`
        // shape with a single `main` entry pointing at `workflow.yaml`.
        assert!(manifest.contains("workflows:"));
        assert!(manifest.contains("name: main"));
        assert!(manifest.contains("path: workflow.ts"));
        assert!(manifest.contains("default: true"));
        assert!(manifest.contains("capabilities: []"));
        assert!(manifest.contains("Keep this aligned with the files matched in workflow.ts."));
    }

    #[test]
    fn create_toml_hybrid_project_uses_js_ast_grep_without_yaml_rules() {
        let temp_dir = tempdir().unwrap();
        let project_path = temp_dir.path().join("toml-hybrid-project");

        let config = ProjectConfig {
            name: "toml-hybrid-project".to_string(),
            description: "TOML hybrid package".to_string(),
            author: "Codemod Team <team@codemod.com>".to_string(),
            license: "MIT".to_string(),
            project_type: ProjectType::Hybrid,
            package_behavior: PackageBehavior::WorkflowOnly,
            language: "toml".to_string(),
            private: false,
            package_manager: Some("pnpm".to_string()),
            git_repository_url: None,
            github_action: false,
            workspace: false,
        };

        create_project(&project_path, &config).unwrap();

        let workflow = fs::read_to_string(project_path.join("workflow.ts")).unwrap();
        assert!(
            workflow.contains("parse<Toml>(file, { language: \"toml\" })"),
            "{workflow}"
        );
        assert!(
            workflow.contains(
                "export default async function main() {\n  await step(\"Setup\", shell(\"$CODEMOD_PATH/scripts/setup.sh\"));\n  await step(\"Transform\", shell(\"$CODEMOD_PATH/scripts/transform.sh\"));\n  await step(\"Pin the package manager\""
            ),
            "{workflow}"
        );
        assert!(!workflow.contains("astGrep"), "{workflow}");
        assert!(!project_path.join("scripts/codemod.ts").exists());

        assert!(!project_path.join("rules/config.yml").exists());
        assert!(project_path.join("tests/fixtures/input.toml").is_file());
        assert!(project_path.join("tests/fixtures/expected.toml").is_file());
    }

    #[test]
    fn create_project_with_skill_generates_workflow_and_skill_assets() {
        let temp_dir = tempdir().unwrap();
        let project_path = temp_dir.path().join("hybrid-project");

        let config = ProjectConfig {
            name: "@codemod/hybrid-project".to_string(),
            description: "Hybrid package".to_string(),
            author: "Codemod Team <team@codemod.com>".to_string(),
            license: "MIT".to_string(),
            project_type: ProjectType::AstGrepJs,
            package_behavior: PackageBehavior::WorkflowAndSkill,
            language: "typescript".to_string(),
            private: false,
            package_manager: Some("npm".to_string()),
            git_repository_url: None,
            github_action: false,
            workspace: false,
        };

        create_project(&project_path, &config).unwrap();
        let manifest = fs::read_to_string(project_path.join("codemod.yaml")).unwrap();
        let skill_root = project_path
            .join(AGENTS_SKILL_ROOT_RELATIVE_PATH)
            .join("hybrid-project");
        let readme = fs::read_to_string(project_path.join("README.md")).unwrap();

        assert!(project_path.join("workflow.ts").is_file());
        assert!(skill_root.join("SKILL.md").is_file());
        assert!(skill_root.join("references/index.md").is_file());
        assert!(manifest.contains("workflows:"));
        assert!(manifest.contains("path: workflow.ts"));
        let workflow = fs::read_to_string(project_path.join("workflow.ts")).unwrap();
        assert!(workflow.contains("installSkill({"));
        assert!(workflow.contains("package: \"@codemod/hybrid-project\""));
        assert!(workflow.contains("path: \"./agents/skill/hybrid-project/SKILL.md\""));
        assert!(readme.contains("## Skill Installation"));
        assert!(readme.contains("npx codemod@latest @codemod/hybrid-project"));
    }

    #[test]
    fn create_project_preserves_selected_package_manager_in_generated_commands() {
        let temp_dir = tempdir().unwrap();
        let project_path = temp_dir.path().join("yarn-project");

        let config = ProjectConfig {
            name: "yarn-project".to_string(),
            description: "Yarn workflow package".to_string(),
            author: "Codemod Team <team@codemod.com>".to_string(),
            license: "MIT".to_string(),
            project_type: ProjectType::AstGrepJs,
            package_behavior: PackageBehavior::WorkflowOnly,
            language: "typescript".to_string(),
            private: false,
            package_manager: Some("yarn".to_string()),
            git_repository_url: None,
            github_action: false,
            workspace: false,
        };

        create_project(&project_path, &config).unwrap();

        let package_json = fs::read_to_string(project_path.join("package.json")).unwrap();
        let readme = fs::read_to_string(project_path.join("README.md")).unwrap();

        assert!(package_json.contains("\"packageManager\": \"yarn@4.x\""));
        assert!(package_json.contains("\"test\": \"codemod workflow test\""));
        let package_json: serde_json::Value = serde_json::from_str(&package_json).unwrap();
        assert_eq!(
            package_json["dependencies"]["codemod"],
            format!("^{}", crate::CLI_VERSION)
        );
        assert!(package_json["devDependencies"].get("codemod").is_none());
        assert!(readme.contains("yarn test"));
        assert!(!readme.contains("npm test"));
    }

    #[test]
    fn local_commands_use_the_installed_codemod() {
        assert_eq!(installed_codemod_command("npm"), "npx codemod");
        assert_eq!(installed_codemod_command("pnpm"), "pnpm codemod");
        assert_eq!(installed_codemod_command("yarn"), "yarn codemod");
        assert_eq!(installed_codemod_command("bun"), "bunx codemod");
        // Someone running a published package has not installed it.
        assert_eq!(
            codemod_cli_command_for_package_manager("npm"),
            "npx codemod@latest"
        );
    }

    #[test]
    fn create_workspace_with_skill_generates_root_readme() {
        let temp_dir = tempdir().unwrap();
        let workspace_path = temp_dir.path().join("workspace");

        let config = ProjectConfig {
            name: "sample-workflow-skill".to_string(),
            description: "Workflow + skill package".to_string(),
            author: "Codemod Team <team@codemod.com>".to_string(),
            license: "MIT".to_string(),
            project_type: ProjectType::AstGrepJs,
            package_behavior: PackageBehavior::WorkflowAndSkill,
            language: "typescript".to_string(),
            private: false,
            package_manager: Some("npm".to_string()),
            git_repository_url: None,
            github_action: false,
            workspace: true,
        };

        create_workspace_project(&workspace_path, &config).unwrap();

        let root_readme = fs::read_to_string(workspace_path.join("README.md")).unwrap();
        assert!(root_readme.contains("## Repository layout"));
        assert!(root_readme.contains("## One-time setup"));
        assert!(root_readme.contains("your organization"));
        assert!(root_readme.contains("Reserve an organization scope in Codemod"));
    }

    #[test]
    fn create_workspace_workflow_only_does_not_generate_root_readme() {
        let temp_dir = tempdir().unwrap();
        let workspace_path = temp_dir.path().join("workspace");

        let config = ProjectConfig {
            name: "workflow-only".to_string(),
            description: "Workflow package".to_string(),
            author: "Codemod Team <team@codemod.com>".to_string(),
            license: "MIT".to_string(),
            project_type: ProjectType::AstGrepJs,
            package_behavior: PackageBehavior::WorkflowOnly,
            language: "typescript".to_string(),
            private: false,
            package_manager: Some("npm".to_string()),
            git_repository_url: None,
            github_action: false,
            workspace: true,
        };

        create_workspace_project(&workspace_path, &config).unwrap();

        assert!(!workspace_path.join("README.md").exists());
    }

    #[test]
    fn create_project_uses_updated_readme_and_workflow_defaults() {
        let temp_dir = tempdir().unwrap();
        let project_path = temp_dir.path().join("workflow-project");

        let config = ProjectConfig {
            name: "workflow-project".to_string(),
            description: "Workflow package".to_string(),
            author: "Codemod Team <team@codemod.com>".to_string(),
            license: "MIT".to_string(),
            project_type: ProjectType::AstGrepJs,
            package_behavior: PackageBehavior::WorkflowOnly,
            language: "typescript".to_string(),
            private: false,
            package_manager: Some("pnpm".to_string()),
            git_repository_url: None,
            github_action: false,
            workspace: false,
        };

        create_project(&project_path, &config).unwrap();

        let readme = fs::read_to_string(project_path.join("README.md")).unwrap();
        assert!(readme
            .contains("Document the exact migration this codemod performs before publishing."));
        assert!(readme.contains("pnpm test"));
        assert!(readme.contains("codemod workflow validate -w workflow.ts"));
        assert!(!readme.contains("Converting `var` declarations to `const`/`let`"));

        let workflow = fs::read_to_string(project_path.join("workflow.ts")).unwrap();
        codemod_workflow::load_workflow(&project_path.join("workflow.ts"))
            .unwrap_or_else(|e| panic!("generated workflow.ts does not build: {e}\n{workflow}"));
        assert!(!workflow.contains("basePath"), "{workflow}");
        assert!(workflow.contains("include: ["));
        assert!(workflow.contains("\"**/*.{ts,tsx,mts,cts}\""));
        assert!(workflow.contains("\"**/node_modules/**\""));

        let package_json = fs::read_to_string(project_path.join("package.json")).unwrap();
        assert!(package_json.contains("\"packageManager\": \"pnpm@10.x\""));

        let gitignore = fs::read_to_string(project_path.join(".gitignore")).unwrap();
        assert!(gitignore.contains("*.tgz\n"));
        assert!(!gitignore.contains("*.tgz "));
    }

    #[test]
    fn every_language_gets_a_callback_workflow_that_builds() {
        let languages = [
            "javascript",
            "typescript",
            "python",
            "rust",
            "go",
            "java",
            "html",
            "xml",
            "css",
            "kotlin",
            "csharp",
            "cpp",
            "c",
            "php",
            "ruby",
            "elixir",
            "json",
            "yaml",
            "toml",
        ];
        for language in languages {
            for project_type in [ProjectType::AstGrepJs, ProjectType::Hybrid] {
                let temp_dir = tempdir().unwrap();
                let project_path = temp_dir.path().join("project");
                let config = ProjectConfig {
                    name: "project".to_string(),
                    description: "Workflow package".to_string(),
                    author: "Codemod Team <team@codemod.com>".to_string(),
                    license: "MIT".to_string(),
                    project_type: project_type.clone(),
                    package_behavior: PackageBehavior::WorkflowAndSkill,
                    language: language.to_string(),
                    private: false,
                    package_manager: Some("npm".to_string()),
                    git_repository_url: None,
                    github_action: false,
                    workspace: false,
                };
                create_project(&project_path, &config).unwrap();

                let path = project_path.join("workflow.ts");
                let workflow = fs::read_to_string(&path).unwrap();
                codemod_workflow::load_workflow(&path).unwrap_or_else(|e| {
                    panic!("{language} {project_type:?}: does not build: {e}\n{workflow}")
                });
                let parse_language = if language == "javascript" || language == "typescript" {
                    language
                } else {
                    &format!("{language}\"")
                };
                assert!(
                    workflow.contains(&format!("(file, {{ language: \"{parse_language}")),
                    "{language}: {workflow}"
                );
                assert!(
                    workflow.contains("forEachFile(files, (file) => {"),
                    "{workflow}"
                );
                assert!(
                    workflow.contains("installSkill({ package: \"project\""),
                    "{workflow}"
                );
                assert!(!project_path.join("scripts/codemod.ts").exists());
                let rules = project_type == ProjectType::Hybrid && !hybrid_without_rules(language);
                assert_eq!(
                    project_path.join("rules/config.yml").is_file(),
                    rules,
                    "{language}"
                );
                assert_eq!(
                    workflow.contains("astGrep("),
                    rules,
                    "{language}: {workflow}"
                );
                let (ext, _, js_expected) = js_fixture(language).unwrap();
                let expected =
                    fs::read_to_string(project_path.join(format!("tests/fixtures/expected.{ext}")))
                        .unwrap();
                match project_type {
                    ProjectType::Hybrid => assert_eq!(
                        expected,
                        hybrid_expected(language).unwrap_or(js_expected),
                        "{language}"
                    ),
                    _ => assert_eq!(expected, js_expected, "{language}"),
                }
                if project_type == ProjectType::Hybrid {
                    let setup = workflow.find("await step(\"Setup\"").unwrap();
                    let own = workflow.find("forEachFile(files").unwrap();
                    let cleanup = workflow.find("await step(\"Cleanup\"").unwrap();
                    assert!(setup < own && own < cleanup, "{workflow}");
                    assert!(
                        workflow
                            .contains("import { astGrep, forEachFile, installSkill, shell, step }")
                            || !rules,
                        "{workflow}"
                    );
                }
            }
        }
    }

    #[test]
    fn create_project_for_xml_generates_xml_fixtures_and_globs() {
        let temp_dir = tempdir().unwrap();
        let project_path = temp_dir.path().join("xml-project");

        let config = ProjectConfig {
            name: "xml-project".to_string(),
            description: "XML workflow package".to_string(),
            author: "Codemod Team <team@codemod.com>".to_string(),
            license: "MIT".to_string(),
            project_type: ProjectType::AstGrepJs,
            package_behavior: PackageBehavior::WorkflowOnly,
            language: "xml".to_string(),
            private: false,
            package_manager: Some("npm".to_string()),
            git_repository_url: None,
            github_action: false,
            workspace: false,
        };

        create_project(&project_path, &config).unwrap();

        assert!(!project_path.join("scripts").exists());
        assert!(project_path.join("tests/fixtures/input.xml").is_file());
        assert!(project_path.join("tests/fixtures/expected.xml").is_file());
        let workflow = fs::read_to_string(project_path.join("workflow.ts")).unwrap();
        assert!(workflow.contains("\"**/*.{xml,csproj,props,targets,config,resx,xaml}\""));
    }

    #[test]
    fn create_project_js_fixture_extension_matches_language() {
        for (language, ext, other_ext) in [("javascript", "js", "ts"), ("typescript", "ts", "js")] {
            let temp_dir = tempdir().unwrap();
            let project_path = temp_dir.path().join(format!("{language}-project"));

            let config = ProjectConfig {
                name: format!("{language}-project"),
                description: "JS workflow package".to_string(),
                author: "Codemod Team <team@codemod.com>".to_string(),
                license: "MIT".to_string(),
                project_type: ProjectType::AstGrepJs,
                package_behavior: PackageBehavior::WorkflowOnly,
                language: language.to_string(),
                private: false,
                package_manager: Some("npm".to_string()),
                git_repository_url: None,
                github_action: false,
                workspace: false,
            };

            create_project(&project_path, &config).unwrap();

            let fixtures = project_path.join("tests/fixtures");
            assert!(
                fixtures.join(format!("input.{ext}")).is_file(),
                "{language}: missing input.{ext}"
            );
            assert!(
                fixtures.join(format!("expected.{ext}")).is_file(),
                "{language}: missing expected.{ext}"
            );
            assert!(!fixtures.join(format!("input.{other_ext}")).exists());
            assert!(!fixtures.join(format!("expected.{other_ext}")).exists());
        }
    }

    #[test]
    fn package_manager_test_command_matches_package_manager() {
        assert_eq!(package_manager_test_command("pnpm"), "pnpm test");
        assert_eq!(package_manager_test_command("yarn"), "yarn test");
        assert_eq!(package_manager_test_command("bun"), "bun run test");
        assert_eq!(package_manager_test_command("npm"), "npm test");
        assert_eq!(package_manager_test_command("unknown"), "npm test");
    }

    #[test]
    fn package_behavior_flags_map_skill_modes() {
        assert_eq!(
            package_behavior_from_flags(false, false).unwrap(),
            PackageBehavior::WorkflowOnly
        );
        assert_eq!(
            package_behavior_from_flags(false, true).unwrap(),
            PackageBehavior::WorkflowAndSkill
        );
        assert_eq!(
            package_behavior_from_flags(true, false).unwrap(),
            PackageBehavior::SkillOnly
        );
        assert!(
            package_behavior_from_flags(true, true).is_err(),
            "--skill + --with-skill should be rejected"
        );
    }

    #[test]
    fn init_help_requires_package_manager_for_all_workflow_project_types() {
        use clap::Args as _;

        // Legacy shell/ast-grep-yaml are normalized to hybrid, which needs a
        // package manager, so the help must not limit the requirement to
        // ast-grep-js/hybrid.
        assert!(
            INIT_AFTER_HELP.contains("for workflow packages (any --project-type, including legacy")
        );
        assert!(INIT_AFTER_HELP.contains("shell/ast-grep-yaml), or with --workspace"));

        let command = Command::augment_args(clap::Command::new("init"));
        let package_manager_help = command
            .get_arguments()
            .find(|arg| arg.get_id() == "package_manager")
            .and_then(|arg| arg.get_help())
            .map(ToString::to_string)
            .unwrap();
        assert!(
            package_manager_help.contains("for workflow packages, i.e. any --project-type"),
            "{package_manager_help}"
        );
    }
}
