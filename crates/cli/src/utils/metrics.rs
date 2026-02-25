use codemod_sandbox::MetricsData;
use inquire::Confirm;

/// Count the total number of metric entries across all metric names
fn count_metric_entries(metrics: &MetricsData) -> usize {
    metrics.values().map(|entries| entries.len()).sum()
}

/// Determine whether the user wants to view the report.
///
/// - `--report` → always true
/// - `--no-interactive` → always false
/// - otherwise → prompt the user, mentioning collected metrics if any
pub fn should_show_report(report_flag: bool, no_interactive: bool, metrics: &MetricsData) -> bool {
    if report_flag {
        return true;
    }
    if no_interactive {
        return false;
    }

    let metric_count = count_metric_entries(metrics);
    let help_message = if metric_count > 0 {
        format!(
            "We collected {} metric data points. Everything is processed offline and stays on your machine.",
            metric_count
        )
    } else {
        "Everything is processed offline and stays on your machine.".to_string()
    };

    Confirm::new("View results in the browser?")
        .with_default(true)
        .with_help_message(&help_message)
        .prompt()
        .unwrap_or(false)
}

/// Print metrics report to stdout if any metrics were collected
pub fn print_metrics(metrics: &MetricsData) {
    let metrics_with_values: Vec<_> = metrics
        .iter()
        .filter(|(_, entries)| !entries.is_empty())
        .collect();

    if metrics_with_values.is_empty() {
        return;
    }

    println!("\nMetrics:");
    for (metric_name, entries) in metrics_with_values {
        println!("  {}:", metric_name);
        let mut sorted_entries: Vec<_> = entries.iter().collect();
        sorted_entries.sort_by(|a, b| b.count.cmp(&a.count));
        for entry in sorted_entries {
            if entry.cardinality.is_empty() {
                // No cardinality dimensions, just show the count
                println!("    {}", entry.count);
            } else {
                // Format cardinality as key=value pairs
                let cardinality_str: String = entry
                    .cardinality
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect::<Vec<_>>()
                    .join(", ");
                println!("    {}: {}", cardinality_str, entry.count);
            }
        }
    }
}
