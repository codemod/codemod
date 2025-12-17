/**
 * Encodes a text string as a valid component of a Uniform Resource Identifier (URI).
 * @param uriComponent A value representing an unencoded URI component.
 */
declare function encodeURIComponent(uriComponent: string | number | boolean): string;

declare function btoa(s: string | Uint8Array): string;
declare function atob(s: string): string;

interface Console {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  log(...data: any[]);
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  error(...data: any[]);
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  warn(...data: any[]);
}

// eslint-disable-next-line no-var
declare var console: Console;
