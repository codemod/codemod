/**
 * Author-facing typing of an inline JSSG transform. Type-only: the runtime
 * never sees a transform function (see `build.ts`), so nothing here executes.
 *
 * `language` selects the ast-grep node types the transform sees, exactly as
 * `import type TSX from "codemod:ast-grep/langs/tsx"` does in a standalone
 * codemod; languages without a published type map fall back to `TypesMap`.
 */
import type Angular from "@codemod.com/jssg-types/langs/angular";
import type C from "@codemod.com/jssg-types/langs/c";
import type CSharp from "@codemod.com/jssg-types/langs/c_sharp";
import type Cpp from "@codemod.com/jssg-types/langs/cpp";
import type Css from "@codemod.com/jssg-types/langs/css";
import type Elixir from "@codemod.com/jssg-types/langs/elixir";
import type Go from "@codemod.com/jssg-types/langs/go";
import type Html from "@codemod.com/jssg-types/langs/html";
import type Java from "@codemod.com/jssg-types/langs/java";
import type JavaScript from "@codemod.com/jssg-types/langs/javascript";
import type JsonLang from "@codemod.com/jssg-types/langs/json";
import type Kotlin from "@codemod.com/jssg-types/langs/kotlin";
import type Php from "@codemod.com/jssg-types/langs/php";
import type Python from "@codemod.com/jssg-types/langs/python";
import type Ruby from "@codemod.com/jssg-types/langs/ruby";
import type Rust from "@codemod.com/jssg-types/langs/rust";
import type Scala from "@codemod.com/jssg-types/langs/scala";
import type Toml from "@codemod.com/jssg-types/langs/toml";
import type Tsx from "@codemod.com/jssg-types/langs/tsx";
import type TypeScript from "@codemod.com/jssg-types/langs/typescript";
import type Xml from "@codemod.com/jssg-types/langs/xml";
import type Yaml from "@codemod.com/jssg-types/langs/yaml";
import type {
  CodemodResult,
  RuleConfig,
  SgRoot,
  StructuredTransformOptions,
  TypesMap,
} from "@codemod.com/jssg-types/main";

/** Engine language names (`src/languages.json`) with a published type map. */
export interface JssgLanguages {
  angular: Angular;
  c: C;
  csharp: CSharp;
  cpp: Cpp;
  css: Css;
  elixir: Elixir;
  go: Go;
  html: Html;
  java: Java;
  javascript: JavaScript;
  json: JsonLang;
  kotlin: Kotlin;
  php: Php;
  python: Python;
  ruby: Ruby;
  rust: Rust;
  scala: Scala;
  toml: Toml;
  tsx: Tsx;
  typescript: TypeScript;
  xml: Xml;
  yaml: Yaml;
}

export type JssgTypes<L extends string> = L extends keyof JssgLanguages
  ? JssgLanguages[L]
  : TypesMap;

/** The element type of an aggregate output: `Finding[]` -> `Finding`. */
type FileOutput<O> = O extends readonly (infer E)[] ? E : unknown;

/**
 * What an inline transform may return, per file: the existing `Codemod`
 * contract (`string | null | undefined`, the string being the new content)
 * or the `StructuredCodemod` contract (`{ content?, output }`, where the
 * present `output` values are aggregated into the command's output array).
 */
export type JssgTransformResult<O> = string | null | undefined | CodemodResult<FileOutput<O>>;

/**
 * The single public transform function of a JSSG definition, with the same
 * `(root, options)` signature as a standalone codemod's default export.
 * Declared through a method so an existing `Codemod<T>` value is assignable.
 */
export type JssgTransform<T extends TypesMap, I, O> = {
  bivariant(
    root: SgRoot<T>,
    options: StructuredTransformOptions<T, I>,
  ): JssgTransformResult<O> | Promise<JssgTransformResult<O>>;
}["bivariant"];

/**
 * Static selector data. Unlike a legacy `getSelector()`, it is plain data
 * declared next to the transform: the executor evaluates it natively before
 * any sandbox starts and skips files it does not match. It never populates
 * `options.matches`; the transform finds its nodes with `root.find`.
 */
export type JssgSelector<T extends TypesMap> = Pick<
  RuleConfig<T>,
  "rule" | "constraints" | "utils"
>;
