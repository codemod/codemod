use codemod_sandbox::MetricsData;

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
