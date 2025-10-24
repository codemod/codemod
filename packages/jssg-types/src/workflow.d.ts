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
export declare function setStepOutput(outputName: string, value: string): void;

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
export declare function getStepOutput(stepId: string, outputName: string): string | null;
