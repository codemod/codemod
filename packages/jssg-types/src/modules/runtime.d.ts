declare module "codemod:runtime" {
  export type RuntimeMeta =
    | string
    | number
    | boolean
    | null
    | { [key: string]: RuntimeMeta | undefined }
    | RuntimeMeta[];

  export interface RuntimeHooks {
    /**
     * Emit a progress event for the current execution context.
     *
     * Progress events are surfaced in task logs and refresh the runtime heartbeat
     * so long-running steps can prove they are still making forward progress.
     */
    progress(message: string, meta?: RuntimeMeta): void;

    /**
     * Emit a non-fatal warning for the current execution context.
     *
     * Warning events are appended to task logs but do not change task state.
     */
    warn(message: string, meta?: RuntimeMeta): void;

    /**
     * Override the current logical execution unit for diagnostics.
     *
     * For file-based transforms, this is typically a relative file path. The
     * runtime uses it to improve watchdog diagnostics and progress attribution.
     */
    setCurrentUnit(unitId: string, meta?: RuntimeMeta): void;

    /**
     * Fail the current file or execution unit immediately.
     *
     * The runtime currently treats this as a terminal task failure.
     */
    failFile(message: string, meta?: RuntimeMeta): never;

    /**
     * Fail the current step immediately.
     *
     * This is always terminal for the running task.
     */
    failStep(message: string, meta?: RuntimeMeta): never;

    /**
     * Return whether the current execution has been canceled.
     *
     * Codemods can poll this to stop cooperatively during long-running work.
     */
    isCanceled(): boolean;
  }

  declare const runtime: RuntimeHooks;

  export default runtime;

  export const progress: RuntimeHooks["progress"];
  export const warn: RuntimeHooks["warn"];
  export const setCurrentUnit: RuntimeHooks["setCurrentUnit"];
  export const failFile: RuntimeHooks["failFile"];
  export const failStep: RuntimeHooks["failStep"];
  export const isCanceled: RuntimeHooks["isCanceled"];
}
