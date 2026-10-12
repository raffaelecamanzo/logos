/*
 * The highlight.js core and its HTML → node-tree parser (S-622, FR-UI-47).
 */

import { describe, expect, it, vi } from "vitest";

import { BUNDLED_LANGUAGES, highlightTree, parseHighlightHtml, type HighlightNode } from "./highlight.ts";

/** Every token class in a tree, flattened. */
function classes(nodes: HighlightNode[]): string[] {
  return nodes.flatMap((n) => (typeof n === "string" ? [] : [n.className, ...classes(n.children)]));
}

/** The tree's text, concatenated — must equal the source byte for byte. */
function text(nodes: HighlightNode[]): string {
  return nodes.map((n) => (typeof n === "string" ? n : text(n.children))).join("");
}

describe("highlightTree", () => {
  it("tokenizes a rust block into class-only spans and keeps the source verbatim", () => {
    const code = 'fn main() {\n    let s = "a < b && c";\n    // done\n}';
    const tree = highlightTree(code, "rust");
    expect(tree).not.toBeNull();
    expect(classes(tree!)).toEqual(expect.arrayContaining(["hljs-keyword", "hljs-string", "hljs-comment"]));
    expect(text(tree!)).toBe(code);
  });

  it("answers to the languages Logos indexes, through their common aliases", () => {
    for (const tag of ["rs", "py", "ts", "tsx", "js", "go", "java", "c", "cpp", "kt", "cs", "rb", "php", "scala",
      "md", "yml", "json", "toml", "dockerfile", "makefile", "sh", "bash", "proto", "graphql", "sql"]) {
      expect(highlightTree("x", tag), tag).not.toBeNull();
    }
  });

  it("renders an unknown, absent or unbundled language plain, and quietly", () => {
    // highlight.js logs an error for every block in a language it was not given;
    // a chat full of `tf` fences must not fill the console on every render.
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    expect(highlightTree("let x = 1;", "brainfuck")).toBeNull();
    expect(highlightTree("let x = 1;", undefined)).toBeNull();
    expect(highlightTree("let x = 1;", "")).toBeNull();
    // Terraform is indexed by Logos but highlight.js ships no grammar for it.
    expect(highlightTree('resource "a" "b" {}', "terraform")).toBeNull();
    expect(error).not.toHaveBeenCalled();
    expect(warn).not.toHaveBeenCalled();
    vi.restoreAllMocks();
  });

  it("bundles exactly the declared language list — the core, never the full build", () => {
    expect([...BUNDLED_LANGUAGES].sort()).toEqual(
      ["bash", "c", "cpp", "csharp", "css", "diff", "dockerfile", "go", "graphql", "ini", "java", "javascript",
        "json", "kotlin", "makefile", "markdown", "php", "protobuf", "python", "ruby", "rust", "scala", "shell",
        "sql", "typescript", "xml", "yaml"],
    );
  });
});

describe("parseHighlightHtml", () => {
  it("decodes the five escapes and nests spans", () => {
    expect(parseHighlightHtml('a<span class="hljs-string">&quot;&lt;&amp;&gt;&#x27;</span>b')).toEqual([
      "a",
      { className: "hljs-string", children: ['"<&>\''] },
      "b",
    ]);
    expect(parseHighlightHtml('<span class="hljs-title function_">f</span>')).toEqual([
      { className: "hljs-title function_", children: ["f"] },
    ]);
    expect(parseHighlightHtml('<span class="a">x<span class="b">y</span>z</span>w')).toEqual([
      { className: "a", children: ["x", { className: "b", children: ["y"] }, "z"] },
      "w",
    ]);
  });

  it("keeps real nested hljs output nested and verbatim (a TS template literal's substitution)", () => {
    const code = "const s = `a ${b} c`;";
    const tree = highlightTree(code, "ts")!;
    expect(text(tree)).toBe(code);
    const template = tree.find((n) => typeof n !== "string" && n.className === "hljs-string");
    expect(template && typeof template !== "string" && classes(template.children)).toContain("hljs-subst");
  });

  it("refuses anything outside highlight.js's closed output grammar", () => {
    for (const html of [
      "<b>x</b>", // another tag
      '<span class="a" style="color:red">x</span>', // an attribute beyond class
      '<span class="a">x', // unclosed
      "x</span>", // unopened
      "&nbsp;", // an entity it never emits
      "a & b", // a bare ampersand it always escapes
      '<span class="a;b">x</span>', // a class list it never emits
    ]) {
      expect(parseHighlightHtml(html), html).toBeNull();
    }
  });
});
