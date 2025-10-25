use inquire::Confirm;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use butterflow_core::execution::CodemodExecutionConfig;
use codemod_llrt_capabilities::types::LlrtSupportedModules;

type CapabilitiesSecurityCallback =
    Arc<Box<dyn Fn(&CodemodExecutionConfig) -> Result<(), anyhow::Error> + Send + Sync>>;

pub fn capabilities_security_callback(no_interaction: bool) -> CapabilitiesSecurityCallback {
    let checked_capabilities = Arc::new(Mutex::new(HashSet::<LlrtSupportedModules>::new()));

    Arc::new(Box::new(move |config: &CodemodExecutionConfig| {
        if no_interaction {
            return Ok(());
        }
        let checked = checked_capabilities.lock().unwrap();
        let need_to_check = config
            .capabilities
            .as_ref()
            .unwrap_or(&HashSet::new())
            .iter()
            .filter(|c| !checked.contains(c))
            .cloned()
            .collect::<Vec<_>>();
        drop(checked);
        if need_to_check.is_empty() {
            return Ok(());
        }
        let answer = Confirm::new(&format!(
            "🛡️  \x1b[31mSecurity Notice\x1b[0m: This action will grant access to `{}`, which may perform sensitive operations. Are you sure you want to continue?", 
            need_to_check.iter().map(|c| format!("{c:?}")).collect::<Vec<_>>().join(", ")
        ))
        .with_default(false)
        .with_help_message("Press 'y' to continue or 'n' to abort")
        .prompt().map_err(|e| anyhow::anyhow!("Failed to get user input: {e}"))?;

        let mut checked = checked_capabilities.lock().unwrap();
        checked.extend(need_to_check);
        drop(checked);

        if !answer {
            return Err(anyhow::anyhow!("Aborting due to capabilities warning"));
        }
        Ok(())
    }))
}
