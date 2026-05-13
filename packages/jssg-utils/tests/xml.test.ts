import { ok as assert } from "assert";
import { parse } from "codemod:ast-grep";
import type Xml from "@codemod.com/jssg-types/langs/xml";
import {
  findElementByKind,
  findElementByTag,
  findElementsByTag,
  getAttributeValue,
  getLineIndent,
  hasTag,
} from "@jssg/utils/xml/elements";

const source = [
  '<Project Sdk="Microsoft.NET.Sdk">',
  "  <PropertyGroup>",
  "    <TargetFramework>net8.0</TargetFramework>",
  "  </PropertyGroup>",
  "  <ItemGroup>",
  '    <PackageReference Include="Newtonsoft.Json" Version="13.0.3" />',
  "  </ItemGroup>",
  "</Project>",
  "",
].join("\n");

const root = parse<Xml>("xml", source).root();

function testFindElementsByTag() {
  const refs = findElementsByTag(root, "PackageReference");

  assert(refs.length === 1, "Should find PackageReference elements");
  assert(refs[0]!.text().includes("Newtonsoft.Json"), "Should return the matching element");
}

function testFindElementByTag() {
  const project = findElementByTag(root, "Project");

  assert(project !== null, "Should find the Project element");
  assert(getAttributeValue(project!, "Sdk") === "Microsoft.NET.Sdk", "Should read Project Sdk");
}

function testFindElementByKind() {
  const project = findElementByTag(root, "Project");
  assert(project !== null, "Should find the Project element");

  const endTag = findElementByKind(project!, "ETag");
  assert(endTag !== null, "Should find a descendant XML end tag");
  assert(endTag!.text().startsWith("</"), "Should return an XML end tag");
}

function testGetAttributeValue() {
  const ref = findElementByTag(root, "PackageReference");
  assert(ref !== null, "Should find PackageReference");

  assert(getAttributeValue(ref!, "Include") === "Newtonsoft.Json", "Should read Include");
  assert(getAttributeValue(ref!, "Version") === "13.0.3", "Should read Version");
  assert(getAttributeValue(ref!, "Missing") === null, "Should return null for missing attributes");
}

function testGetAttributeValueIgnoresDescendantAttributes() {
  const nested = parse<Xml>(
    "xml",
    [
      '<Project Sdk="Microsoft.NET.Sdk">',
      '  <ItemGroup Include="outer">',
      '    <PackageReference Include="inner" />',
      "  </ItemGroup>",
      "</Project>",
    ].join("\n"),
  ).root();
  const project = findElementByTag(nested, "Project");
  const itemGroup = findElementByTag(nested, "ItemGroup");

  assert(project !== null, "Should find Project");
  assert(itemGroup !== null, "Should find ItemGroup");
  assert(getAttributeValue(project!, "Include") === null, "Should not read descendant attributes");
  assert(getAttributeValue(itemGroup!, "Include") === "outer", "Should read direct tag attributes");
}

function testHasTag() {
  assert(hasTag(root, "TargetFramework"), "Should detect present tags");
  assert(!hasTag(root, "TargetFrameworks"), "Should reject absent tags");
}

function testGetLineIndent() {
  const ref = findElementByTag(root, "PackageReference");
  assert(ref !== null, "Should find PackageReference");

  assert(getLineIndent(source, ref!) === "    ", "Should return leading indentation");

  const inlineSource = "<Project><PropertyGroup /></Project>";
  const inlineRoot = parse<Xml>("xml", inlineSource).root();
  const propertyGroup = findElementByTag(inlineRoot, "PropertyGroup");
  assert(propertyGroup !== null, "Should find inline PropertyGroup");
  assert(getLineIndent(inlineSource, propertyGroup!) === "", "Should ignore non-whitespace prefix");
}

testFindElementsByTag();
testFindElementByTag();
testFindElementByKind();
testGetAttributeValue();
testGetAttributeValueIgnoresDescendantAttributes();
testHasTag();
testGetLineIndent();
