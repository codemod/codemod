/**
 * Metrics Module
 * 
 * This module provides functions to track and report metrics during codemod execution.
 * Metrics are useful for collecting statistics about transformations, such as:
 * - Number of nodes processed
 * - Number of transformations applied
 * - Custom counters for specific patterns
 * 
 * Metrics are automatically collected and displayed at the end of codemod execution.
 */

/**
 * A metric object that provides getter and setter methods for a metric value.
 * 
 * Metrics are scoped by name, so multiple metrics can be tracked independently.
 * The value is a non-negative integer (usize).
 */
export interface Metric {
  /**
   * Gets the current value of the metric.
   * 
   * @returns The current metric value (defaults to 0 if not set)
   * 
   * @example
   * ```typescript
   * const count = myMetric.get();
   * console.log(`Current count: ${count}`);
   * ```
   */
  get(): number;

  /**
   * Sets the metric value.
   * 
   * @param value - The new metric value (must be a non-negative integer)
   * 
   * @example
   * ```typescript
   * myMetric.set(10);
   * myMetric.set(myMetric.get() + 1); // Increment
   * ```
   */
  set(value: number): void;
}

/**
 * Creates or retrieves a metric with the given scope name.
 * 
 * If the metric doesn't exist, it will be initialized with the optional initial value.
 * If no initial value is provided, it defaults to 0.
 * 
 * Metrics are shared across all files processed in a single codemod execution,
 * so you can track cumulative statistics.
 * 
 * @param scopeName - A unique name for the metric scope
 * @param initial - Optional initial value (defaults to 0)
 * @returns A Metric object with get() and set() methods
 * 
 * @example
 * ```typescript
 * import { useMetric } from 'codemod:metrics';
 * 
 * // Initialize a metric with a starting value
 * const processedCount = useMetric('processed', 0);
 * 
 * // Initialize a metric without an initial value (defaults to 0)
 * const errorCount = useMetric('errors');
 * 
 * // Use metrics in your transform
 * export default async function transform(root) {
 *   const nodes = root.root().findAll({ rule: { pattern: 'console.log' } });
 *   
 *   processedCount.set(processedCount.get() + nodes.length);
 *   
 *   nodes.forEach(node => {
 *     // ... process node ...
 *     processedCount.set(processedCount.get() + 1);
 *   });
 *   
 *   return root.root().commitEdits(edits);
 * }
 * ```
 * 
 * @example
 * ```typescript
 * // Metrics are cumulative across all files
 * const totalNodes = useMetric('total_nodes', 0);
 * 
 * export default async function transform(root) {
 *   const count = root.root().children().length;
 *   totalNodes.set(totalNodes.get() + count);
 *   return null; // No changes
 * }
 * ```
 */
export declare function useMetric(scopeName: string, initial?: number): Metric;

