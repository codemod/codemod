use inquire::Confirm;
use log::error;
use std::sync::{Arc, Mutex};

use butterflow_core::execution::CodemodExecutionConfig;

type CapabilitiesSecurityCallback = Arc<Box<dyn Fn(&CodemodExecutionConfig) + Send + Sync>>;

pub fn capabilities_security_callback() -> CapabilitiesSecurityCallback {
    let checked_capabilities = Arc::new(Mutex::new(Vec::<String>::new()));

    Arc::new(Box::new(move |config: &CodemodExecutionConfig| {
        for capability in config.capabilities.as_ref().unwrap_or(&Vec::new()) {
            let answer = Confirm::new(&format!(
            "🛡️  \x1b[31mSecurity Notice\x1b[0m: This action will grant access to `{capability}`, which may perform sensitive operations. Are you sure you want to continue?"
        ))
        .with_default(false)
        .with_help_message("Press 'y' to continue or 'n' to abort")
        .prompt().unwrap_or_else(|e| {
            error!("Failed to get user input: {e}");
            std::process::exit(1);
        });

            if !answer {
                error!("Aborting due to capabilities warning");
                std::process::exit(1);
            } else {
                let mut checked_capabilities = checked_capabilities.lock().unwrap();
                checked_capabilities.push(capability.to_string());
            }
        }
    }))
}
