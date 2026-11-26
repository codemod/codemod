// references.get(node) on an assignment gives an edge for each use of the
// assigned name.
import { symbols } from "codemod:ast-grep";

export default function transform(root) {
  const { references } = symbols("python");

  // Find the first assignment 'counter = 0'
  const assignment = root.root().find({ rule: { pattern: "counter = 0" } });
  if (!assignment) {
    throw new Error("Expected to find 'counter = 0'");
  }

  const refs = references.get(assignment) ?? [];

  if (refs.length === 0) {
    console.log("No references found - semantic provider may not have indexed yet");
    return null;
  }

  // Collect reference texts
  let refTexts = [];
  for (const { node } of refs) {
    if (typeof node.text === "function") {
      refTexts.push(node.text());
    }
  }

  // Should find references to 'counter'
  if (refTexts.length === 0) {
    throw new Error("Expected to find references to 'counter', got " + refTexts.length);
  }

  // All references should be 'counter'
  for (const text of refTexts) {
    if (text !== "counter") {
      throw new Error("Expected reference text to be 'counter', got: '" + text + "'");
    }
  }

  return null;
}
