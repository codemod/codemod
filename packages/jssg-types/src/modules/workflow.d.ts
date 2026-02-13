declare module "codemod:workflow" {
  /**
   * Workflow Step Outputs Module
   *
   * This module provides functions to store and retrieve step outputs
   * across workflow executions, similar to GitHub Actions.
   */

  /**
   * Sets a step output value.
   *
   * Step outputs allow you to share data between steps in a workflow.
   * Similar to GitHub Actions, you can reference outputs like:
   * ${{ steps.my_step.outputs.my_output }}
   *
   * In native mode, outputs are appended to the file specified by
   * the STEP_OUTPUTS environment variable.
   *
   * In WASM mode, outputs are stored in memory.
   *
   * @param stepId - The unique identifier of the step
   * @param outputName - The name of the output variable
   * @param value - The value to store (as a string)
   *
   * @example
   * ```typescript
   * import { setStepOutput } from 'codemod:workflow';
   *
   * // In a step with id="build"
   * setStepOutput('build', 'version', '1.2.3');
   * setStepOutput('build', 'artifact_path', '/tmp/build/app.zip');
   *
   * // Later steps can access these via:
   * // ${{ steps.build.outputs.version }}
   * // ${{ steps.build.outputs.artifact_path }}
   * ```
   */
  export function setStepOutput(outputName: string, value: string): void;

  /**
   * Gets a step output value.
   *
   * Returns the value as a string if found, or null if the output doesn't exist.
   *
   * @param stepId - The unique identifier of the step
   * @param outputName - The name of the output variable
   * @returns The value as a string, or null if not found
   *
   * @example
   * ```typescript
   * import { getStepOutput } from 'codemod:workflow';
   *
   * // Get output from a previous step
   * const version = getStepOutput('build', 'version');
   * console.log(version); // '1.2.3'
   *
   * // Check if output exists
   * const artifactPath = getStepOutput('build', 'artifact_path');
   * if (artifactPath) {
   *   console.log(`Artifact ready at: ${artifactPath}`);
   * }
   * ```
   */
  export function getStepOutput(stepId: string, outputName: string): string | null;

  /**
   * Gets a step output value, or sets it if it doesn't exist (atomically).
   *
   * This function is useful in concurrent scenarios where multiple files are processed
   * in parallel. The first thread to call this function will set the value if it doesn't
   * exist, and subsequent calls will return the existing value.
   *
   * This operation is atomic, preventing race conditions when multiple threads try to
   * access/set the same output simultaneously.
   *
   * @param stepId - The unique identifier of the step
   * @param outputName - The name of the output variable
   * @param defaultValue - The value to set if the output doesn't exist
   * @returns The existing value (if found) or the newly set default value
   *
   * @example
   * ```typescript
   * import { getOrSetStepOutput, setStepOutput } from 'codemod:workflow';
   *
   * // Multiple files being processed in parallel
   * async function transform(root: SgRoot<JS>): Promise<string | null> {
   *   const source = root.root().text();
   *   const newFiles = source.split("\n").filter(line => line.trim() !== "");
   *
   *   // Get existing files or initialize with empty array
   *   const existingFilesStr = getOrSetStepOutput("scan-files", "allFiles", "[]");
   *   const existingFiles = JSON.parse(existingFilesStr);
   *
   *   // Merge with existing files
   *   const allFiles = [...existingFiles, ...newFiles];
   *
   *   // Update the output
   *   setStepOutput("allFiles", JSON.stringify(allFiles));
   *
   *   return null;
   * }
   * ```
   */
  export function getOrSetStepOutput(
    stepId: string,
    outputName: string,
    defaultValue: string,
  ): string;
}
