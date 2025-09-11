use inquire::Confirm;
use log::error;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use butterflow_core::execution::CodemodExecutionConfig;
use codemod_llrt_capabilities::module_builder::LlrtSupportedModules;

type CapabilitiesSecurityCallback = Arc<Box<dyn Fn(&CodemodExecutionConfig) + Send + Sync>>;

pub fn capabilities_security_callback() -> CapabilitiesSecurityCallback {
    let checked_capabilities = Arc::new(Mutex::new(HashSet::<LlrtSupportedModules>::new()));

    Arc::new(Box::new(move |config: &CodemodExecutionConfig| {
        let checked = checked_capabilities.lock().unwrap();
        let need_to_check = config
            .capabilities
            .as_ref()
            .unwrap_or(&Vec::new())
            .iter()
            .filter(|c| !checked.contains(c))
            .cloned()
            .collect::<Vec<_>>();
        drop(checked);
        if need_to_check.is_empty() {
            return;
        }
        let answer = Confirm::new(&format!(
            "🛡️  \x1b[31mSecurity Notice\x1b[0m: This action will grant access to `{}`, which may perform sensitive operations. Are you sure you want to continue?", 
            need_to_check.iter().map(|c| format!("{c:?}")).collect::<Vec<_>>().join(", ")
        ))
        .with_default(false)
        .with_help_message("Press 'y' to continue or 'n' to abort")
        .prompt().unwrap_or_else(|e| {
            error!("Failed to get user input: {e}");
            std::process::exit(1);
        });

        let mut checked = checked_capabilities.lock().unwrap();
        checked.extend(need_to_check);
        drop(checked);

        if !answer {
            error!("Aborting due to capabilities warning");
            std::process::exit(1);
        }
    }))
}
