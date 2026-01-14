/**
 * A metric atom for tracking labeled counts.
 *
 * MetricAtom provides a way to increment and retrieve counts for different labels
 * within a named metric. This is useful for tracking things like prop usage counts,
 * component instances, etc.
 *
 * @example
 * ```ts
 * import { useMetricAtom } from "codemod:metrics";
 *
 * const propUsage = useMetricAtom("prop-usage");
 *
 * export function transform(root) {
 *   root.findAll({ rule: { pattern: "title={$_}" } }).forEach(() => {
 *     propUsage.increment("title");
 *   });
 *   return null;
 * }
 * ```
 */
export declare class MetricAtom {
  /**
   * The name of this metric.
   */
  readonly name: string;

  /**
   * Increment the count for a given label.
   *
   * @param label - The label to increment (e.g., "title", "placeholder", "Button")
   * @param amount - The amount to increment by (default: 1)
   *
   * @example
   * ```ts
   * propUsage.increment("title");       // increment by 1
   * propUsage.increment("title", 5);    // increment by 5
   * ```
   */
  increment(label: string, amount?: number): void;

  /**
   * Get the current values for all labels in this metric.
   *
   * @returns An object mapping labels to their current counts
   *
   * @example
   * ```ts
   * const values = propUsage.getValues();
   * // { "title": 10, "placeholder": 5 }
   * ```
   */
  getValues(): Record<string, number>;
}

/**
 * Create or retrieve a metric atom with the given name.
 *
 * Multiple calls with the same name will return atoms that share the same
 * underlying data, allowing metrics to be collected across multiple files.
 *
 * @param name - The name of the metric (e.g., "prop-usage", "component-count")
 * @returns A MetricAtom instance for tracking labeled counts
 *
 * @example
 * ```ts
 * import { useMetricAtom } from "codemod:metrics";
 *
 * const propUsage = useMetricAtom("prop-usage");
 * const componentCount = useMetricAtom("component-count");
 * ```
 */
export declare function useMetricAtom(name: string): MetricAtom;
