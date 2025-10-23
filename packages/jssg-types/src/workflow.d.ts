/**
 * Workflow Global Module
 * 
 * This module provides functions to store and retrieve global variables
 * across workflow executions.
 */

/**
 * Sets a global variable by name and value.
 * 
 * In native mode, the variable is appended to the file specified by
 * the WORKFLOW_GLOBAL environment variable.
 * 
 * In WASM mode, the variable is stored in memory.
 * 
 * @param name - The name of the variable to set
 * @param variable - The value to store (as a string)
 * 
 * @example
 * ```typescript
 * import { setGlobalVariable } from 'codemod:workflow';
 * 
 * // Store a string
 * setGlobalVariable('userName', 'John Doe');
 * 
 * // Store a number (as string)
 * setGlobalVariable('count', '42');
 * 
 * // Store JSON
 * setGlobalVariable('user', JSON.stringify({ name: 'Jane', age: 30 }));
 * ```
 */
export declare function setGlobalVariable(name: string, variable: string ): void;

/**
 * Gets a global variable by name.
 * 
 * Returns the value as a string if found, or null if the variable doesn't exist.
 * 
 * The function automatically detects the type of stored data:
 * - JSON objects and arrays are parsed and stringified back
 * - Numbers (integers and floats) are returned as strings
 * - Regular strings are returned as-is
 * 
 * @param name - The name of the variable to retrieve
 * @returns The value as a string, or null if not found
 * 
 * @example
 * ```typescript
 * import { getGlobalVariable } from 'codemod:workflow';
 * 
 * // Get a string
 * const name = getGlobalVariable('userName');
 * console.log(name); // 'John Doe'
 * 
 * // Get a number (parse if needed)
 * const count = getGlobalVariable('count');
 * const numericCount = parseInt(count || '0');
 * 
 * // Get JSON (parse the result)
 * const userData = getGlobalVariable('user');
 * if (userData) {
 *   const user = JSON.parse(userData);
 *   console.log(user.name); // 'Jane'
 * }
 * 
 * // Non-existent variable
 * const missing = getGlobalVariable('nonExistent');
 * console.log(missing); // null
 * ```
 */
export declare function getGlobalVariable(name: string): string;
