import type { Edit } from "@codemod.com/jssg-types/main";
import type { SgRoot } from "codemod:ast-grep";
import type TSX from "codemod:ast-grep/langs/tsx";
import {
  importBindingOf,
  renameReferences,
  resolvesToFactory,
} from "../../src/javascript/exports/bindings.ts";
import { getImport, updateImport } from "../../src/javascript/exports/imports.ts";

const FACTORIES = ["setup", "setupWorker", "Client"];
const METHODS = new Set(["stop", "query", "listen"]);

async function transform(root: SgRoot<TSX>): Promise<string | null> {
  const rootNode = root.root();
  const filename = root.filename();
  const edits: Edit[] = [];

  const cleanUrl = getImport(rootNode, { type: "named", name: "cleanUrl", from: "lib" });
  if (cleanUrl && !cleanUrl.isNamespace) {
    edits.push(...renameReferences(cleanUrl.node, "getCleanUrlString", filename));
    const renamed = updateImport(rootNode, {
      type: "named",
      from: "lib",
      specifiers: [{ name: "cleanUrl", to: "getCleanUrlString" }],
    });
    if (renamed) edits.push(renamed);
  }

  const calls = rootNode.findAll({ rule: { kind: "call_expression" } });
  for (const call of calls) {
    const callee = call.field("function");
    if (!callee) continue;

    if (callee.kind() === "identifier") {
      const binding = importBindingOf(callee);
      if (!binding || binding.source !== "lib") continue;

      if (binding.importedName === "setup") {
        edits.push(call.replace(`hit(${JSON.stringify(binding.localName)})`));
      } else if (binding.importedName === "default") {
        edits.push(call.replace(`hitDefault(${JSON.stringify(binding.localName)})`));
      } else if (binding.importedName === "*") {
        edits.push(call.replace(`hitNamespace(${JSON.stringify(binding.localName)})`));
      }
      continue;
    }

    if (callee.kind() !== "member_expression") continue;
    const object = callee.field("object");
    const property = callee.field("property");
    if (!object || object.kind() !== "identifier" || !property || !METHODS.has(property.text())) {
      continue;
    }

    const hit = FACTORIES.some((name) => resolvesToFactory(object, name, ["lib"]));
    if (!hit) continue;

    edits.push({
      startPos: call.range().start.index,
      endPos: call.range().start.index,
      insertedText: "/*factory*/ ",
    });
  }

  if (edits.length === 0) return null;
  return rootNode.commitEdits(edits);
}

export default transform;
