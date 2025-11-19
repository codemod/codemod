/**
 * Formats a string into a URL-friendly slug
 * Matches the slugify function from utils/strings/index.ts
 * Converts to lowercase, removes special characters, replaces spaces with hyphens
 */
export function formatSlug(str: string): string {
  if (!str || typeof str !== "string") {
    return "";
  }

  const acceptedCharacters = [
    "a-z", // lower-case letters
    "0-9", // numbers
    " ", // spaces
    "\\-", // hyphens (escaped)
  ];

  return str
    .toString()
    .normalize("NFD") // Split accented characters
    .replace(/[\u0300-\u036f]/g, "") // Remove accents
    .toLowerCase()
    .replace(new RegExp(`[^${acceptedCharacters.join("")}]`, "g"), "") // Remove special characters
    .replace(/\s+/g, "-") // Replace spaces with hyphens
    .trim();
}
