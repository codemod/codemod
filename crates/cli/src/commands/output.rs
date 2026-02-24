use crate::commands::harness_adapter::{HarnessAdapterError, OutputFormat};
use serde::Serialize;
use std::path::Path;

#[derive(Serialize)]
struct InstallErrorEnvelope {
    ok: bool,
    code: String,
    exit_code: i32,
    message: String,
    hint: String,
}

pub(crate) fn exit_adapter_error(error: HarnessAdapterError, format: OutputFormat) -> ! {
    let envelope = InstallErrorEnvelope {
        ok: false,
        code: error.code().to_string(),
        exit_code: error.exit_code(),
        message: error.to_string(),
        hint: error.hint().to_string(),
    };

    match format {
        OutputFormat::Json => match serde_json::to_string_pretty(&envelope) {
            Ok(json) => println!("{json}"),
            Err(_) => eprintln!("{}: {}", envelope.code, envelope.message),
        },
        OutputFormat::Yaml => match serde_yaml::to_string(&envelope) {
            Ok(yaml) => println!("{yaml}"),
            Err(_) => eprintln!("{}: {}", envelope.code, envelope.message),
        },
        OutputFormat::Logs | OutputFormat::Table => {
            eprintln!("Error [{}]: {}", envelope.code, envelope.message);
            eprintln!("Hint: {}", envelope.hint);
        }
    }

    std::process::exit(envelope.exit_code);
}

pub(crate) fn format_output_path(path: &Path) -> String {
    if let Ok(current_dir) = std::env::current_dir() {
        if let Ok(relative_path) = path.strip_prefix(current_dir) {
            return relative_path.display().to_string();
        }
    }

    if let Some(home_dir) = dirs::home_dir() {
        if let Ok(home_relative_path) = path.strip_prefix(home_dir) {
            return format!("~/{}", home_relative_path.display());
        }
    }

    path.display().to_string()
}
