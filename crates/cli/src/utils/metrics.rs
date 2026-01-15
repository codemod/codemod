use codemod_sandbox::MetricsData;

/// Print metrics report to stdout if any metrics were collected
pub fn print_metrics(metrics: &MetricsData) {
    let metrics_with_values: Vec<_> = metrics
        .iter()
        .filter(|(_, labels)| !labels.is_empty())
        .collect();

    if metrics_with_values.is_empty() {
        return;
    }

    println!("\nMetrics:");
    for (metric_name, labels) in metrics_with_values {
        println!("  {}:", metric_name);
        let mut sorted_labels: Vec<_> = labels.iter().collect();
        sorted_labels.sort_by(|a, b| b.1.cmp(a.1));
        for (label, count) in sorted_labels {
            println!("    {}: {}", label, count);
        }
    }
}
