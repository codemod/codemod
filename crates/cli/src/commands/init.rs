use anyhow::{anyhow, Result};
use clap::Args;
use console::{style, Emoji};
use inquire::{Confirm, Select, Text};
use log::info;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

#[derive(Args, Debug)]
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

struct ProjectConfig {
    name: String,
    description: String,
    author: String,
    license: String,
    project_type: ProjectType,
    language: String,
    private: bool,
    package_manager: Option<String>,
    git_repository_url: Option<String>,
    github_action: bool,
    workspace: bool,
}

// Template constants using include_str!
const CODEMOD_TEMPLATE: &str = include_str!("../templates/codemod.yaml");
const SHELL_WORKFLOW_TEMPLATE: &str = include_str!("../templates/shell/workflow.yaml");
const JS_ASTGREP_WORKFLOW_TEMPLATE: &str = include_str!("../templates/js-astgrep/workflow.yaml");
const ASTGREP_YAML_WORKFLOW_TEMPLATE: &str =
    include_str!("../templates/astgrep-yaml/workflow.yaml");
const HYBRID_WORKFLOW_TEMPLATE: &str = include_str!("../templates/hybrid/workflow.yaml");
const GITIGNORE_TEMPLATE: &str = include_str!("../templates/common/.gitignore");
const README_TEMPLATE: &str = include_str!("../templates/common/README.md");
const GITHUB_ACTION_TEMPLATE: &str = include_str!("../templates/common/publish.yml");
const GITHUB_ACTION_WORKSPACE_TEMPLATE: &str =
    include_str!("../templates/common/publish-workspace.yml");

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
const JS_TSCONFIG_TEMPLATE: &str = include_str!("../templates/js-astgrep/tsconfig.json");
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
        let project_type = args
            .project_type
            .clone()
            .ok_or_else(|| anyhow!("Project type is required --project-type"))?;
        let normalized_project_type = match project_type {
            ProjectType::Shell | ProjectType::AstGrepYaml => {
                println!(
                    "{} Deprecated project type selected; scaffolding a Hybrid (Shell + YAML + jssg) package",
                    style("ℹ").cyan(),
                );
                ProjectType::Hybrid
            }
            other => other,
        };
        let package_manager = match (
            &normalized_project_type,
            args.package_manager.clone(),
            args.workspace,
        ) {
            (ProjectType::AstGrepJs, Some(pm), _) | (ProjectType::Hybrid, Some(pm), _) => Some(pm),
            (_, Some(pm), true) => Some(pm), // Workspace mode always needs package manager
            (ProjectType::AstGrepJs, None, _) | (ProjectType::Hybrid, None, _) => {
                return Err(anyhow!(
                    "--package-manager is required when --project-type is ast-grep-js or hybrid"
                ));
            }
            (_, None, true) => {
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
            project_type: normalized_project_type.clone(),
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

fn interactive_setup(project_name: &str, args: &Command) -> Result<ProjectConfig> {
    println!(
        "{} {}",
        ROCKET,
        style("Creating a new codemod project").bold()
    );
    println!();

    // Project type selection
    let project_type = if let Some(pt) = &args.project_type {
        pt.clone()
    } else {
        select_project_type()?
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

    let package_manager = if args.package_manager.is_some() {
        args.package_manager.clone()
    } else {
        Some(
            Select::new(
                "Which package manager would you like to use?",
                vec!["npm", "pnpm", "bun", "yarn"],
            )
            .prompt()?
            .to_string(),
        )
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
        "Other",
    ];

    let selection = Select::new("Which language would you like to target?", options).prompt()?;

    let language = match selection {
        "JavaScript/TypeScript" => "typescript",
        "Python" => "python",
        "Rust" => "rust",
        "Go" => "go",
        "Java" => "java",
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

    // Create workflow.yaml
    create_workflow(project_path, config)?;

    // Create project-specific structure
    match config.project_type {
        ProjectType::Shell => create_shell_project(project_path, config)?,
        ProjectType::AstGrepJs => create_js_astgrep_project(project_path, config)?,
        ProjectType::AstGrepYaml => create_astgrep_yaml_project(project_path, config)?,
        ProjectType::Hybrid => create_hybrid_project(project_path, config)?,
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

    let manifest_content = CODEMOD_TEMPLATE
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
    let workflow_content = match config.project_type {
        ProjectType::Shell => SHELL_WORKFLOW_TEMPLATE,
        ProjectType::AstGrepJs => JS_ASTGREP_WORKFLOW_TEMPLATE,
        ProjectType::AstGrepYaml => ASTGREP_YAML_WORKFLOW_TEMPLATE,
        ProjectType::Hybrid => HYBRID_WORKFLOW_TEMPLATE,
    }
    .replace("{language}", &config.language);

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
    }

    Ok(())
}

fn create_gitignore(project_path: &Path) -> Result<()> {
    fs::write(project_path.join(".gitignore"), GITIGNORE_TEMPLATE)?;
    Ok(())
}

fn create_readme(project_path: &Path, config: &ProjectConfig) -> Result<()> {
    let test_command = match config.project_type {
        ProjectType::Shell => "bash scripts/transform.sh",
        ProjectType::AstGrepJs => "npm test",
        ProjectType::AstGrepYaml => "ast-grep test rules/",
        ProjectType::Hybrid => "npm test",
    };

    let readme_content = README_TEMPLATE
        .replace("{name}", &config.name)
        .replace("{description}", &config.description)
        .replace("{language}", &config.language)
        .replace("{test_command}", test_command)
        .replace("{license}", &config.license);

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

    // Create workflow.yaml
    create_workflow(codemod_path, config)?;

    // Create project-specific structure
    match config.project_type {
        ProjectType::Shell => create_shell_project(codemod_path, config)?,
        ProjectType::AstGrepJs => create_js_astgrep_project(codemod_path, config)?,
        ProjectType::AstGrepYaml => create_astgrep_yaml_project(codemod_path, config)?,
        ProjectType::Hybrid => create_hybrid_project(codemod_path, config)?,
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
        println!("{CHECKMARK} Generated workflow.yaml definition");
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
