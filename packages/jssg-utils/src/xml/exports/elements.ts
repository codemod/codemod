import type Xml from "@codemod.com/jssg-types/langs/xml";
import type { Edit, Kinds, Rule, SgNode } from "@codemod.com/jssg-types/main";

export type XmlNode = SgNode<Xml, Kinds<Xml>>;

export const elementByTagRule = (tag: string): Rule<Xml> => ({
  kind: "element",
  has: {
    any: [
      {
        kind: "STag",
        has: {
          kind: "Name",
          pattern: tag,
        },
      },
      {
        kind: "EmptyElemTag",
        has: {
          kind: "Name",
          pattern: tag,
        },
      },
    ],
  },
});

export function findElementsByTag(root: XmlNode, tag: string): XmlNode[] {
  return root.findAll({
    rule: elementByTagRule(tag),
  });
}

export function findElementByTag(node: XmlNode, tag: string): XmlNode | null {
  return node.find({
    rule: elementByTagRule(tag),
  });
}

export function findElementByKind(node: XmlNode, kind: keyof Xml): XmlNode | null {
  return node.find({
    rule: {
      kind,
    },
  });
}

export function getAttributeValue(element: XmlNode, attrName: string): string | null {
  const tag = element.children().find((child) => child.is("STag") || child.is("EmptyElemTag"));
  const attr = tag
    ?.children()
    .find(
      (child) =>
        child.is("Attribute") &&
        child.children().some((attrChild) => attrChild.is("Name") && attrChild.text() === attrName),
    );
  const text = attr
    ?.children()
    .find((child) => child.is("AttValue"))
    ?.text();

  return text ? text.slice(1, text.length - 1) : null;
}

export function hasTag(root: XmlNode, tag: string): boolean {
  return findElementsByTag(root, tag).length > 0;
}

export function getLineIndent(src: string, node: XmlNode): string {
  const start = node.range().start.index;
  let lineStart = start;
  while (lineStart > 0 && src[lineStart - 1] !== "\n") {
    lineStart--;
  }
  const linePrefix = src.slice(lineStart, start);
  const indent = linePrefix.match(/^[ \t]*/)?.[0];
  return indent ?? "";
}

export function deleteNodeLine(src: string, node: XmlNode): Edit {
  const range = node.range();
  let startPos = range.start.index;
  while (startPos > 0 && src[startPos - 1] !== "\n") {
    startPos--;
  }
  let endPos = range.start.index;
  while (endPos < src.length && src[endPos] !== "\n") {
    endPos++;
  }
  if (endPos < src.length) endPos++;
  return { startPos, endPos, insertedText: "" };
}
