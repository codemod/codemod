use crate::utils::skill_layout::{
    expected_authored_skill_relative_file, AGENTS_SKILL_ROOT_RELATIVE_PATH,
};
use anyhow::{anyhow, Result};
use clap::Args;
use console::{style, Emoji};
use inquire::{Confirm, Select, Text};
use log::info;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

#[derive(Args, Debug, Default)]
pub struct Command {
    /// Project directory name
    #[arg(value_name = "PATH")]
    path: Option<PathBuf>,

    /// Project name (defaults to directory name)
    #[arg(long)]
    name: Option<String>,

    /// Git repository URL
    #[arg(long)]
    git_repository_url: Option<String>,

    /// Project type
    #[arg(long)]
    project_type: Option<ProjectType>,

    /// Scaffold a skill-focused package with an install-skill workflow
    #[arg(long, conflicts_with = "with_skill")]
    skill: bool,

    /// Also scaffold skill behavior alongside workflow files
    #[arg(long, conflicts_with = "skill")]
    with_skill: bool,

    /// Package manager
    #[arg(long)]
    package_manager: Option<String>,

    /// Target language
    #[arg(long)]
    language: Option<String>,

    /// Project description
    #[arg(long)]
    description: Option<String>,

    /// Author name and email
    #[arg(long)]
    author: Option<String>,

    /// License
    #[arg(long)]
    license: Option<String>,

    /// Make package private
    #[arg(long)]
    private: bool,

    /// Overwrite existing files
    #[arg(long)]
    force: bool,

    /// Use defaults without prompts
    #[arg(long)]
    no_interactive: bool,

    /// Create GitHub Actions workflow for publishing
    #[arg(long)]
    github_action: bool,

    /// Create a monorepo workspace structure
    #[arg(long)]
    workspace: bool,
}

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
const JS_ASTGREP_WORKFLOW_TEMPLATE: &str = include_str!("../templates/js-astgrep/workflow.yaml");
const ASTGREP_YAML_WORKFLOW_TEMPLATE: &str =
    include_str!("../templates/astgrep-yaml/workflow.yaml");
const HYBRID_WORKFLOW_TEMPLATE: &str = include_str!("../templates/hybrid/workflow.yaml");
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
const JS_APPLY_SCRIPT_FOR_JAVASCRIPT: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.ts.ts");
const JS_APPLY_SCRIPT_FOR_PYTHON: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.py.ts");
const JS_APPLY_SCRIPT_FOR_RUST: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.rs.ts");
const JS_APPLY_SCRIPT_FOR_GO: &str = include_str!("../templates/js-astgrep/scripts/codemod.go.ts");
const JS_APPLY_SCRIPT_FOR_JAVA: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.java.ts");
const JS_APPLY_SCRIPT_FOR_HTML: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.html.ts");
const JS_APPLY_SCRIPT_FOR_CSS: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.css.ts");
const JS_APPLY_SCRIPT_FOR_KOTLIN: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.kt.ts");
const JS_APPLY_SCRIPT_FOR_ANGULAR: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.angular.ts");
const JS_APPLY_SCRIPT_FOR_CSHARP: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.cs.ts");
const JS_APPLY_SCRIPT_FOR_CPP: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.cpp.ts");
const JS_APPLY_SCRIPT_FOR_C: &str = include_str!("../templates/js-astgrep/scripts/codemod.c.ts");
const JS_APPLY_SCRIPT_FOR_PHP: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.php.ts");
const JS_APPLY_SCRIPT_FOR_RUBY: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.rb.ts");
const JS_APPLY_SCRIPT_FOR_ELIXIR: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.ex.ts");
const JS_APPLY_SCRIPT_FOR_JSON: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.json.ts");
const JS_APPLY_SCRIPT_FOR_YAML: &str =
    include_str!("../templates/js-astgrep/scripts/codemod.yaml.ts");
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
const CSS_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.css");
const CSS_TEST_EXPECTED: &str = include_str!("../templates/js-astgrep/tests/fixtures/expected.css");
const KOTLIN_TEST_INPUT: &str = include_str!("../templates/js-astgrep/tests/fixtures/input.kt");
const KOTLIN_TEST_EXPECTED: &str =
    include_str!("../templates/js-astgrep/tests/fixtures/expected.kt");
const ANGULAR_TEST_INPUT: &str =
    include_str!("../templates/js-astgrep/tests/fixtures/input.angular.html");
const ANGULAR_TEST_EXPECTED: &str =
    include_str!("../templates/js-astgrep/tests/fixtures/expected.angular.html");
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
const ASTGREP_PATTERNS_FOR_CSS: &str =
    include_str!("../templates/astgrep-yaml/rules/config.css.yml");
const ASTGREP_PATTERNS_FOR_KOTLIN: &str =
    include_str!("../templates/astgrep-yaml/rules/config.kt.yml");
const ASTGREP_PATTERNS_FOR_ANGULAR: &str =
    include_str!("../templates/astgrep-yaml/rules/config.angular.yml");
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
                    "--package-manager is required when --project-type is ast-grep-js or hybrid"
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
        "CSS",
        "Kotlin",
        "Angular",
        "C#",
        "C++",
        "C",
        "PHP",
        "Ruby",
        "Elixir",
        "Json",
        "Yaml",
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
        "CSS" => "css",
        "Kotlin" => "kotlin",
        "Angular" => "angular",
        "C#" => "csharp",
        "C++" => "cpp",
        "C" => "c",
        "PHP" => "php",
        "Ruby" => "ruby",
        "Elixir" => "elixir",
        "Json" => "json",
        "Yaml" => "yaml",
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

    // Always create workflow.yaml (workflow-first package model)
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
    let mut workflow_content = if config.package_behavior == PackageBehavior::SkillOnly {
        SKILL_WORKFLOW_TEMPLATE
            .replace("{name}", &config.name)
            .replace("{skill_path}", &default_skill_path)
    } else {
        match config.project_type {
            ProjectType::Shell => SHELL_WORKFLOW_TEMPLATE,
            ProjectType::AstGrepJs => JS_ASTGREP_WORKFLOW_TEMPLATE,
            ProjectType::AstGrepYaml => ASTGREP_YAML_WORKFLOW_TEMPLATE,
            ProjectType::Hybrid => HYBRID_WORKFLOW_TEMPLATE,
        }
        .replace("{language}", &config.language)
    };

    if config.package_behavior == PackageBehavior::WorkflowAndSkill {
        workflow_content.push_str(
            &INSTALL_SKILL_NODE_TEMPLATE
                .replace("{name}", &config.name)
                .replace("{skill_path}", &default_skill_path),
        );
    }

    fs::write(project_path.join("workflow.yaml"), workflow_content)?;
    Ok(())
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

fn create_js_astgrep_project(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    let codemod_command = if let Some(package_manager) = &config.package_manager {
        match package_manager.as_str() {
            "npm" => "npx codemod@latest",
            "yarn" => "yarn dlx codemod@latest",
            "pnpm" => "pnpm dlx codemod@latest",
            "bun" => "bunx codemod@latest",
            _ => "npx codemod@latest",
        }
    } else {
        "npx codemod@latest"
    };
    // Create package.json
    let package_json = JS_PACKAGE_JSON_TEMPLATE
        .replace("{name}", &config.name)
        .replace("{description}", &config.description)
        .replace("{codemod_command}", codemod_command);

    fs::write(project_path.join("package.json"), package_json)?;

    // Create scripts directory
    let scripts_dir = project_path.join("scripts");
    fs::create_dir_all(&scripts_dir)?;

    let codemod_script = match config.language.as_str() {
        "javascript" | "typescript" => JS_APPLY_SCRIPT_FOR_JAVASCRIPT.to_string(),
        "python" => JS_APPLY_SCRIPT_FOR_PYTHON.to_string(),
        "rust" => JS_APPLY_SCRIPT_FOR_RUST.to_string(),
        "go" => JS_APPLY_SCRIPT_FOR_GO.to_string(),
        "java" => JS_APPLY_SCRIPT_FOR_JAVA.to_string(),
        "html" => JS_APPLY_SCRIPT_FOR_HTML.to_string(),
        "css" => JS_APPLY_SCRIPT_FOR_CSS.to_string(),
        "kotlin" => JS_APPLY_SCRIPT_FOR_KOTLIN.to_string(),
        "angular" => JS_APPLY_SCRIPT_FOR_ANGULAR.to_string(),
        "csharp" => JS_APPLY_SCRIPT_FOR_CSHARP.to_string(),
        "cpp" => JS_APPLY_SCRIPT_FOR_CPP.to_string(),
        "c" => JS_APPLY_SCRIPT_FOR_C.to_string(),
        "php" => JS_APPLY_SCRIPT_FOR_PHP.to_string(),
        "ruby" => JS_APPLY_SCRIPT_FOR_RUBY.to_string(),
        "elixir" => JS_APPLY_SCRIPT_FOR_ELIXIR.to_string(),
        "json" => JS_APPLY_SCRIPT_FOR_JSON.to_string(),
        "yaml" => JS_APPLY_SCRIPT_FOR_YAML.to_string(),
        _ => JS_APPLY_SCRIPT_FOR_JAVASCRIPT.to_string(),
    };
    fs::write(scripts_dir.join("codemod.ts"), codemod_script.as_str())?;

    // Create tsconfig.json
    fs::write(project_path.join("tsconfig.json"), JS_TSCONFIG_TEMPLATE)?;

    // Create tests
    create_js_tests(project_path, config)?;

    Ok(())
}

fn create_astgrep_yaml_project(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    // Create rules directory
    let rules_dir = project_path.join("rules");
    fs::create_dir_all(&rules_dir)?;

    let config_file = match config.language.as_str() {
        "javascript" | "typescript" => ASTGREP_PATTERNS_FOR_JAVASCRIPT,
        "python" => ASTGREP_PATTERNS_FOR_PYTHON,
        "rust" => ASTGREP_PATTERNS_FOR_RUST,
        "go" => ASTGREP_PATTERNS_FOR_GO,
        "java" => ASTGREP_PATTERNS_FOR_JAVA,
        "html" => ASTGREP_PATTERNS_FOR_HTML,
        "css" => ASTGREP_PATTERNS_FOR_CSS,
        "kotlin" => ASTGREP_PATTERNS_FOR_KOTLIN,
        "angular" => ASTGREP_PATTERNS_FOR_ANGULAR,
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

    // Copy jssg codemod script
    let codemod_script = match config.language.as_str() {
        "javascript" | "typescript" => JS_APPLY_SCRIPT_FOR_JAVASCRIPT,
        "python" => JS_APPLY_SCRIPT_FOR_PYTHON,
        "rust" => JS_APPLY_SCRIPT_FOR_RUST,
        "go" => JS_APPLY_SCRIPT_FOR_GO,
        "java" => JS_APPLY_SCRIPT_FOR_JAVA,
        "html" => JS_APPLY_SCRIPT_FOR_HTML,
        "css" => JS_APPLY_SCRIPT_FOR_CSS,
        "kotlin" => JS_APPLY_SCRIPT_FOR_KOTLIN,
        "angular" => JS_APPLY_SCRIPT_FOR_ANGULAR,
        "csharp" => JS_APPLY_SCRIPT_FOR_CSHARP,
        "cpp" => JS_APPLY_SCRIPT_FOR_CPP,
        "c" => JS_APPLY_SCRIPT_FOR_C,
        "php" => JS_APPLY_SCRIPT_FOR_PHP,
        "ruby" => JS_APPLY_SCRIPT_FOR_RUBY,
        "elixir" => JS_APPLY_SCRIPT_FOR_ELIXIR,
        "json" => JS_APPLY_SCRIPT_FOR_JSON,
        "yaml" => JS_APPLY_SCRIPT_FOR_YAML,
        _ => JS_APPLY_SCRIPT_FOR_JAVASCRIPT,
    };
    fs::write(scripts_dir.join("codemod.ts"), codemod_script)?;

    // Create rules directory
    let rules_dir = project_path.join("rules");
    fs::create_dir_all(&rules_dir)?;

    let config_file = match config.language.as_str() {
        "javascript" | "typescript" => ASTGREP_PATTERNS_FOR_JAVASCRIPT,
        "python" => ASTGREP_PATTERNS_FOR_PYTHON,
        "rust" => ASTGREP_PATTERNS_FOR_RUST,
        "go" => ASTGREP_PATTERNS_FOR_GO,
        "java" => ASTGREP_PATTERNS_FOR_JAVA,
        "html" => ASTGREP_PATTERNS_FOR_HTML,
        "css" => ASTGREP_PATTERNS_FOR_CSS,
        "kotlin" => ASTGREP_PATTERNS_FOR_KOTLIN,
        "angular" => ASTGREP_PATTERNS_FOR_ANGULAR,
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
    fs::create_dir_all(tests_dir.join("fixtures"))?;

    if config.language == "javascript" || config.language == "typescript" {
        fs::write(tests_dir.join("fixtures").join("input.js"), JS_TEST_INPUT)?;
        fs::write(
            tests_dir.join("fixtures").join("expected.js"),
            JS_TEST_EXPECTED,
        )?;
    }

    // Create package.json and tsconfig.json at project root
    let package_json_content = format!(
        r#"{{
  "name": "{}",
  "version": "1.0.0",
  "description": "{}",
  "main": "scripts/codemod.ts",
  "scripts": {{
    "test": "node scripts/codemod.ts"
  }},
  "dependencies": {{
    "codemod:ast-grep": "latest"
  }},
  "devDependencies": {{
    "@types/node": "^20.0.0",
    "typescript": "^5.0.0"
  }}
}}"#,
        config.name, config.description
    );

    let tsconfig_content = r#"{
  "compilerOptions": {
    "target": "ES2020",
    "module": "commonjs",
    "lib": ["ES2020"],
    "outDir": "./dist",
    "rootDir": "./",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "forceConsistentCasingInFileNames": true,
    "moduleResolution": "node",
    "allowSyntheticDefaultImports": true,
    "experimentalDecorators": true,
    "emitDecoratorMetadata": true
  },
  "include": ["scripts/**/*"],
  "exclude": ["node_modules", "dist"]
}"#;

    fs::write(project_path.join("package.json"), package_json_content)?;
    fs::write(project_path.join("tsconfig.json"), tsconfig_content)?;

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

fn create_js_tests(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    let tests_dir = project_path.join("tests");
    fs::create_dir_all(tests_dir.join("fixtures"))?;

    if config.language == "javascript" || config.language == "typescript" {
        fs::write(tests_dir.join("fixtures").join("input.js"), JS_TEST_INPUT)?;
        fs::write(
            tests_dir.join("fixtures").join("expected.js"),
            JS_TEST_EXPECTED,
        )?;
    } else if config.language == "python" {
        fs::write(
            tests_dir.join("fixtures").join("input.py"),
            PYTHON_TEST_INPUT,
        )?;
        fs::write(
            tests_dir.join("fixtures").join("expected.py"),
            PYTHON_TEST_EXPECTED,
        )?;
    } else if config.language == "rust" {
        fs::write(tests_dir.join("fixtures").join("input.rs"), RUST_TEST_INPUT)?;
        fs::write(
            tests_dir.join("fixtures").join("expected.rs"),
            RUST_TEST_EXPECTED,
        )?;
    } else if config.language == "go" {
        fs::write(tests_dir.join("fixtures").join("input.go"), GO_TEST_INPUT)?;
        fs::write(
            tests_dir.join("fixtures").join("expected.go"),
            GO_TEST_EXPECTED,
        )?;
    } else if config.language == "java" {
        fs::write(
            tests_dir.join("fixtures").join("input.java"),
            JAVA_TEST_INPUT,
        )?;
        fs::write(
            tests_dir.join("fixtures").join("expected.java"),
            JAVA_TEST_EXPECTED,
        )?;
    } else if config.language == "csharp" {
        fs::write(
            tests_dir.join("fixtures").join("input.cs"),
            CSHARP_TEST_INPUT,
        )?;
        fs::write(
            tests_dir.join("fixtures").join("expected.cs"),
            CSHARP_TEST_EXPECTED,
        )?;
    } else if config.language == "cpp" {
        fs::write(tests_dir.join("fixtures").join("input.cpp"), CPP_TEST_INPUT)?;
        fs::write(
            tests_dir.join("fixtures").join("expected.cpp"),
            CPP_TEST_EXPECTED,
        )?;
    } else if config.language == "c" {
        fs::write(tests_dir.join("fixtures").join("input.c"), C_TEST_INPUT)?;
        fs::write(
            tests_dir.join("fixtures").join("expected.c"),
            C_TEST_EXPECTED,
        )?;
    } else if config.language == "php" {
        fs::write(tests_dir.join("fixtures").join("input.php"), PHP_TEST_INPUT)?;
        fs::write(
            tests_dir.join("fixtures").join("expected.php"),
            PHP_TEST_EXPECTED,
        )?;
    } else if config.language == "ruby" {
        fs::write(tests_dir.join("fixtures").join("input.rb"), RUBY_TEST_INPUT)?;
        fs::write(
            tests_dir.join("fixtures").join("expected.rb"),
            RUBY_TEST_EXPECTED,
        )?;
    } else if config.language == "elixir" {
        fs::write(
            tests_dir.join("fixtures").join("input.ex"),
            ELIXIR_TEST_INPUT,
        )?;
        fs::write(
            tests_dir.join("fixtures").join("expected.ex"),
            ELIXIR_TEST_EXPECTED,
        )?;
    } else if config.language == "html" {
        fs::write(
            tests_dir.join("fixtures").join("input.html"),
            HTML_TEST_INPUT,
        )?;
        fs::write(
            tests_dir.join("fixtures").join("expected.html"),
            HTML_TEST_EXPECTED,
        )?;
    } else if config.language == "css" {
        fs::write(tests_dir.join("fixtures").join("input.css"), CSS_TEST_INPUT)?;
        fs::write(
            tests_dir.join("fixtures").join("expected.css"),
            CSS_TEST_EXPECTED,
        )?;
    } else if config.language == "kotlin" {
        fs::write(
            tests_dir.join("fixtures").join("input.kt"),
            KOTLIN_TEST_INPUT,
        )?;
        fs::write(
            tests_dir.join("fixtures").join("expected.kt"),
            KOTLIN_TEST_EXPECTED,
        )?;
    } else if config.language == "angular" {
        fs::write(
            tests_dir.join("fixtures").join("input-angular.ts"),
            ANGULAR_TEST_INPUT,
        )?;
        fs::write(
            tests_dir.join("fixtures").join("expected-angular.ts"),
            ANGULAR_TEST_EXPECTED,
        )?;
    } else if config.language == "json" {
        fs::write(
            tests_dir.join("fixtures").join("input.json"),
            JSON_TEST_INPUT,
        )?;
        fs::write(
            tests_dir.join("fixtures").join("expected.json"),
            JSON_TEST_EXPECTED,
        )?;
    } else if config.language == "yaml" {
        fs::write(
            tests_dir.join("fixtures").join("input.yaml"),
            YAML_TEST_INPUT,
        )?;
        fs::write(
            tests_dir.join("fixtures").join("expected.yaml"),
            YAML_TEST_EXPECTED,
        )?;
    }

    Ok(())
}

fn create_gitignore(project_path: &Path) -> Result<()> {
    fs::write(project_path.join(".gitignore"), GITIGNORE_TEMPLATE)?;
    Ok(())
}

fn create_readme(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    let test_command = if config.package_behavior == PackageBehavior::SkillOnly {
        format!("npx codemod@latest {}", config.name)
    } else {
        match config.project_type {
            ProjectType::Shell => "bash scripts/transform.sh".to_string(),
            ProjectType::AstGrepJs => "npm test".to_string(),
            ProjectType::AstGrepYaml => "ast-grep test rules/".to_string(),
            ProjectType::Hybrid => "npm test".to_string(),
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
npx codemod@latest {}
```
"#,
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

    // Always create workflow.yaml (workflow-first package model)
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
            let package_manager = config.package_manager.clone().unwrap_or("npm".to_string());

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
                            style("npm install").cyan()
                        );
                    }
                }
                Err(e) => {
                    println!("{} npm not found: {}", style("⚠").red(), e);
                    println!(
                        "  You can run {} manually later",
                        style("npm install").cyan()
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
            println!("{CHECKMARK} Generated workflow.yaml definition");
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
            style(format!("npx codemod@latest {}", config.name)).dim()
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
        // Determine the path to the workflow.yaml
        let workflow_path = if config.workspace {
            format!(
                "{}/codemods/{}/workflow.yaml",
                project_path.display(),
                codemod_dir_name
            )
        } else {
            format!("{}/workflow.yaml", project_path.display())
        };

        println!();
        println!("  {}", style("Validate your workflow").bold().cyan());
        println!(
            "  {}",
            style(format!(
                "npx codemod@latest workflow validate -w {}",
                workflow_path
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
                "npx codemod@latest workflow run -w {} --target ./some/target/path",
                workflow_path
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
                style(format!("npx codemod@latest {}", config.name)).dim()
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
        assert!(project_path.join("workflow.yaml").is_file());

        let manifest = fs::read_to_string(project_path.join("codemod.yaml")).unwrap();
        assert!(manifest.contains("capabilities:"));
        assert!(manifest.contains("workflow: \"workflow.yaml\""));
        let parsed_manifest: CodemodManifest = serde_yaml::from_str(&manifest).unwrap();
        let validation = validate_skill_behavior(&project_path, &parsed_manifest).unwrap();
        assert_eq!(validation.linked_reference_count, 1);
        let workflow = fs::read_to_string(project_path.join("workflow.yaml")).unwrap();
        assert!(workflow.contains("install-skill:"));
        assert!(workflow.contains("package: \"@codemod/sample-skill\""));
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
        assert!(codemod_path.join("workflow.yaml").is_file());
        let manifest = fs::read_to_string(codemod_path.join("codemod.yaml")).unwrap();
        let parsed_manifest: CodemodManifest = serde_yaml::from_str(&manifest).unwrap();
        let validation = validate_skill_behavior(&codemod_path, &parsed_manifest).unwrap();
        assert_eq!(validation.linked_reference_count, 1);
        let workflow = fs::read_to_string(codemod_path.join("workflow.yaml")).unwrap();
        assert!(workflow.contains("install-skill:"));
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
            package_manager: Some("npm".to_string()),
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
        assert!(manifest.contains("path: workflow.yaml"));
        assert!(manifest.contains("default: true"));
        assert!(manifest.contains("capabilities: []"));
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

        assert!(project_path.join("workflow.yaml").is_file());
        assert!(skill_root.join("SKILL.md").is_file());
        assert!(skill_root.join("references/index.md").is_file());
        assert!(manifest.contains("workflows:"));
        assert!(manifest.contains("path: workflow.yaml"));
        let workflow = fs::read_to_string(project_path.join("workflow.yaml")).unwrap();
        assert!(workflow.contains("install-skill:"));
        assert!(workflow.contains("package: \"@codemod/hybrid-project\""));
        assert!(workflow.contains("path: \"./agents/skill/hybrid-project/SKILL.md\""));
        assert!(readme.contains("## Skill Installation"));
        assert!(readme.contains("npx codemod@latest @codemod/hybrid-project"));
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
}
