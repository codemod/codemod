export const VSCODE_PREFIX = "vscode://";
export const CURSOR_PREFIX = "cursor://";

export const CUSTOMER_STORY_TAG = {
  value: "customer-stories",
  label: "Customer Stories",
} as const;

export const REGISTRY_FILTER_TYPES = {
  useCase: "category",
  framework: "framework",
  owner: "author",
} as const;

export const STATIC_HEADER_ROUTES = ["/registry"];

// For /studio
export const LEARN_KEY = "learn";
export const ACCESS_TOKEN_REQUESTED_BY_VSCE_STORAGE_KEY_DEPRECATED =
  "accessTokenRequested"; // For backwards-compatibility
export const ACCESS_TOKEN_REQUESTED_BY_CLI_STORAGE_KEY =
  "accessTokenRequestedByCLI";
export const ACCESS_TOKEN_COMMANDS = [
  ACCESS_TOKEN_REQUESTED_BY_CLI_STORAGE_KEY,
] as const;
export const TWO_MINS_IN_MS = 2 * 60 * 1000;
