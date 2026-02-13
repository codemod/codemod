declare module "codemod:metrics" {
  /**
   * Cardinality dimensions for a metric entry.
   * Keys and values must be strings. Undefined/null values are omitted.
   */
  export type Cardinality = Record<string, string | undefined | null>;

  /**
   * A metric entry with its cardinality dimensions and count.
   */
  export interface MetricEntry {
    cardinality: Record<string, string>;
    count: number;
  }

  /**
   * A metric atom for tracking counts with cardinality dimensions.
   *
   * MetricAtom provides a way to increment and retrieve counts for different
   * cardinality combinations within a named metric. This is useful for tracking
   * things like prop usage counts with multiple dimensions (e.g., propName + propValue).
   *
   * @example
   * ```ts
   * import { useMetricAtom } from "codemod:metrics";
   *
   * const propUsage = useMetricAtom("prop-usage");
   *
   * export function transform(root) {
   *   root.findAll({ rule: { pattern: "$PROP={$_}" } }).forEach((match) => {
   *     propUsage.increment({
   *       propName: match.getMatch("PROP")?.text(),
   *     });
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
     * Increment the count for a given cardinality combination.
     *
     * @param cardinality - Object with dimension key-value pairs. Undefined/null values are omitted.
     * @param amount - The amount to increment by (default: 1)
     *
     * @example
     * ```ts
     * // Simple increment with no cardinality
     * metric.increment();
     *
     * // Single dimension
     * propUsage.increment({ propName: "title" });
     *
     * // Multiple dimensions
     * propUsage.increment({ propName: "className", propValue: "container" });
     *
     * // With amount
     * propUsage.increment({ propName: "title" }, 5);
     *
     * // Undefined values are omitted
     * propUsage.increment({ propName: "title", propValue: undefined });
     * // Results in cardinality: { propName: "title" }
     * ```
     */
    increment(cardinality?: Cardinality, amount?: number): void;

    /**
     * Get the current entries for this metric.
     *
     * @returns An array of entries with their cardinality and count
     *
     * @example
     * ```ts
     * const entries = propUsage.getEntries();
     * // [
     * //   { cardinality: { propName: "title" }, count: 10 },
     * //   { cardinality: { propName: "className", propValue: "container" }, count: 5 }
     * // ]
     * ```
     */
    getEntries(): MetricEntry[];
  }

  /**
   * Create or retrieve a metric atom with the given name.
   *
   * Multiple calls with the same name will return atoms that share the same
   * underlying data, allowing metrics to be collected across multiple files.
   *
   * @param name - The name of the metric (e.g., "prop-usage", "component-count")
   * @returns A MetricAtom instance for tracking counts with cardinality
   *
   * @example
   * ```ts
   * import { useMetricAtom } from "codemod:metrics";
   *
   * const propUsage = useMetricAtom("prop-usage");
   * const componentCount = useMetricAtom("component-count");
   * ```
   */
  export function useMetricAtom(name: string): MetricAtom;
}
