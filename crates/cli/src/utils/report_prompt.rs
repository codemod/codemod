use inquire::Confirm;

/// Determine whether the user wants to view the report.
///
/// - `--report` → always true
/// - `--no-interactive`, or no terminal to ask on → always false
/// - otherwise → prompt the user, when files were modified
pub fn should_show_report(report_flag: bool, no_interactive: bool, files_modified: usize) -> bool {
    if report_flag {
        return true;
    }
    if !crate::utils::interactive::is_interactive(no_interactive) || files_modified == 0 {
        return false;
    }
    Confirm::new("View results in the browser?")
        .with_default(true)
        .with_help_message("Everything is processed offline and stays on your machine.")
        .prompt()
        .unwrap_or(false)
}
