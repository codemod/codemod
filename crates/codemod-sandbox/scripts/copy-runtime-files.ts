import { constants } from "node:fs";
import { access, copyFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const WASM_BUILD_DIR = resolve(SCRIPT_DIR, "..", "..", "..", "target", "wasm-bindgen");

interface FileCopyTask {
  readonly sourceFileName: string;
  readonly destinationPath: string[];
}

const FILE_MAPPINGS: ReadonlyArray<FileCopyTask> = [
  {
    sourceFileName: "codemod-sandbox_bg.js",
    destinationPath: ["js", "factory.js"],
  },
  {
    sourceFileName: "codemod-sandbox_bg.wasm",
    destinationPath: ["sandbox.wasm"],
  },
] as const;

async function validateFileExists(filePath: string): Promise<boolean> {
  try {
    await access(filePath, constants.F_OK);
    return true;
  } catch {
    return false;
  }
}

async function performFileCopy(task: FileCopyTask): Promise<void> {
  const sourcePath = resolve(WASM_BUILD_DIR, task.sourceFileName);
  const destinationPath = resolve(SCRIPT_DIR, "..", ...task.destinationPath);

  const fileExists = await validateFileExists(sourcePath);

  if (!fileExists) {
    console.warn(`WARNING: Codemod Sandbox bits weren't found at:\n${sourcePath}`);
    process.exit(0);
  }

  console.log(`Copying\nfrom: "${sourcePath}"\nto: "${destinationPath}"`);
  await copyFile(sourcePath, destinationPath);
}

async function executeOperations(): Promise<void> {
  const copyOperations = FILE_MAPPINGS.map(performFileCopy);
  await Promise.all(copyOperations);
  console.log("Done copying runtime files");
}

await executeOperations();
