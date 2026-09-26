import { readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const exportsRoot = join(root, "packages/jssg-utils/src");
const docsDir = join(root, "docs/community/jssg/utils");
const docsLabel = "docs/community/jssg/utils/";

const declarationPattern =
  /^export\s+(?:async\s+)?(?:function\*?|const|let|class|enum)\s+([A-Za-z0-9_]+)/gm;
const exportListPattern = /export\s+\{([^}]+)\}\s*(?:from\s+["'][^"']+["'])?\s*;?/g;

function readNames(pattern, source) {
  const names = new Set();
  for (const match of source.matchAll(pattern)) {
    names.add(match[1]);
  }
  return names;
}

function readExportListNames(source) {
  const names = new Set();
  for (const match of source.matchAll(exportListPattern)) {
    for (const part of match[1].split(",")) {
      const trimmed = part.trim();
      if (!trimmed) continue;
      const sides = trimmed.split(/\s+as\s+/);
      const exported = (sides[1] ?? sides[0]).trim();
      if (exported) names.add(exported);
    }
  }
  return names;
}

function collectExportsByLanguage() {
  const byLanguage = new Map();
  for (const languageDir of readdirSync(exportsRoot)) {
    const exportsDir = join(exportsRoot, languageDir, "exports");
    let entries;
    try {
      if (!statSync(exportsDir).isDirectory()) continue;
      entries = readdirSync(exportsDir);
    } catch {
      continue;
    }

    const names = new Set();
    for (const fileName of entries) {
      if (!fileName.endsWith(".ts")) continue;
      const source = readFileSync(join(exportsDir, fileName), "utf8");
      for (const name of readNames(declarationPattern, source)) names.add(name);
      for (const name of readExportListNames(source)) names.add(name);
    }
    if (names.size > 0) byLanguage.set(languageDir, names);
  }
  return byLanguage;
}

function collectHeadings(source) {
  return readNames(/^#{2,3} `([^`]+)`\s*$/gm, source);
}

function formatList(names) {
  return [...names]
    .sort()
    .map((name) => `  - ${name}`)
    .join("\n");
}

function collectDocsHeadingsByPage() {
  const byPage = new Map();
  for (const fileName of readdirSync(docsDir)) {
    if (!fileName.endsWith(".mdx")) continue;
    const page = fileName.slice(0, -".mdx".length);
    byPage.set(page, collectHeadings(readFileSync(join(docsDir, fileName), "utf8")));
  }
  return byPage;
}

const exportsByLanguage = collectExportsByLanguage();
const headingsByPage = collectDocsHeadingsByPage();
const errors = [];
let exportCount = 0;

for (const [language, names] of exportsByLanguage) {
  exportCount += names.size;
  const headings = headingsByPage.get(language) ?? new Set();
  const missingHeadings = [...names].filter((name) => !headings.has(name));
  if (missingHeadings.length > 0) {
    errors.push(
      `These ${language} exports have no ## or ### \`name\` heading in ${docsLabel}${language}.mdx:\n${formatList(missingHeadings)}`,
    );
  }
}

for (const [page, headings] of headingsByPage) {
  const names = exportsByLanguage.get(page) ?? new Set();
  const headingsWithoutExport = [...headings].filter((name) => !names.has(name));
  if (headingsWithoutExport.length > 0) {
    errors.push(
      `${docsLabel}${page}.mdx has headings that are not exported from packages/jssg-utils/src/${page}/exports:\n${formatList(headingsWithoutExport)}`,
    );
  }
}

if (errors.length > 0) {
  console.error(errors.join("\n\n"));
  process.exit(1);
}

console.log(`jssg utils docs match ${exportCount} exports.`);
