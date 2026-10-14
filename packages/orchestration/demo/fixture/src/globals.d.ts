// The SDK under migration. Declarations are excluded from every step.

/** @deprecated Use `newApi({ name })`. */
declare function oldApi(name: string): string;

/** Accepts the bare name while the migration is in progress. */
declare function newApi(options: string | { name: string }): string;
