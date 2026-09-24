/*
 * Unit tests for the Config editor's TOML line-patcher (S-191, FR-UI-12) — the
 * deterministic typed-field ⇄ raw-TOML round-trip. Pure string-in/string-out, so
 * no DOM is needed. Mirrors the legacy `config-editor.js` patch semantics: replace
 * in place, insert after an existing header, create an absent table, and remove a
 * cleared optional key.
 */

import { describe, expect, it } from "vitest";

import { dropEmptyTable, patch, tomlValue } from "./toml.ts";

describe("tomlValue — typed-field serialisation (S-191, FR-UI-12)", () => {
  it("encodes a non-empty list, and removes the key when cleared", () => {
    expect(tomlValue("list", "rust\npython")).toBe('["rust", "python"]');
    // A cleared list removes the key (revert to default), never `key = []`.
    expect(tomlValue("list", "   \n  ")).toBeNull();
  });

  it("encodes a string scalar quoted, and removes it when blank", () => {
    expect(tomlValue("str", "claude-x")).toBe('"claude-x"');
    expect(tomlValue("str", "  ")).toBeNull();
  });

  it("passes int/float through trimmed, removing on blank", () => {
    expect(tomlValue("int", " 42 ")).toBe("42");
    expect(tomlValue("float", "0.85")).toBe("0.85");
    expect(tomlValue("int", "")).toBeNull();
  });

  it("maps the bool tri-state (true/false/unset)", () => {
    expect(tomlValue("bool", "true")).toBe("true");
    expect(tomlValue("bool", "false")).toBe("false");
    expect(tomlValue("bool", "")).toBeNull();
  });
});

describe("patch — typed field into the raw candidate (S-191, FR-UI-12)", () => {
  it("replaces an existing top-level key in place", () => {
    const raw = 'languages = ["rust"]\nmax_file_size = 1048576\n';
    expect(patch(raw, "", "max_file_size", "int", "2097152")).toBe(
      'languages = ["rust"]\nmax_file_size = 2097152\n',
    );
  });

  it("replaces a key inside its owning [table], not a same-named top-level key", () => {
    const raw = 'model = "top"\n\n[chat]\nprovider = "openai"\nmodel = "old"\n';
    const next = patch(raw, "chat", "model", "str", "claude-x");
    expect(next).toContain('[chat]\nprovider = "openai"\nmodel = "claude-x"');
    // The top-level same-named key is untouched (region-scoped replace).
    expect(next).toContain('model = "top"');
  });

  it("inserts a new key right after an existing table header", () => {
    const raw = '[chat]\nprovider = "openai"\n';
    expect(patch(raw, "chat", "model", "str", "claude-x")).toBe(
      '[chat]\nmodel = "claude-x"\nprovider = "openai"\n',
    );
  });

  it("creates an absent table when the key has nowhere to go", () => {
    const raw = 'languages = ["rust"]\n';
    const next = patch(raw, "constraints", "max_cc", "int", "10");
    expect(next).toContain("[constraints]");
    expect(next).toContain("max_cc = 10");
  });

  it("removes a cleared optional key (revert to default), leaving the rest intact", () => {
    const raw = '[constraints]\nmax_cc = 10\nmax_fn_lines = 80\n';
    expect(patch(raw, "constraints", "max_cc", "int", "")).toBe(
      '[constraints]\nmax_fn_lines = 80\n',
    );
  });

  it("round-trips: a value set then cleared returns the original document", () => {
    const raw = '[chat]\nprovider = "openai"\n';
    const set = patch(raw, "chat", "model", "str", "claude-x");
    const cleared = patch(set, "chat", "model", "str", "");
    expect(cleared).toBe(raw);
  });
});

describe("patch — a multi-line array value is one value (S-430, FR-UI-38)", () => {
  // The exact shape `logos init --workspace` writes (`toml::to_string_pretty`):
  // the dominant real-world `members`, so it is the fixture, not a tidy one-liner.
  const INIT_WRITTEN = [
    "[workspace]",
    'name = "shop"',
    "members = [",
    '    "api",',
    '    "web",',
    "]",
    'default = "api"',
    "",
    "[workspace.warm]",
    "concurrency = 2",
  ].join("\n");

  it("replaces every line of the array, leaving no dangling tail", () => {
    const out = patch(INIT_WRITTEN, "workspace", "members", "list", "api\nweb\nworker");
    expect(out).toBe(
      [
        "[workspace]",
        'name = "shop"',
        'members = ["api", "web", "worker"]',
        'default = "api"',
        "",
        "[workspace.warm]",
        "concurrency = 2",
      ].join("\n"),
    );
  });

  it("removes every line of the array when the field is cleared", () => {
    const out = patch(INIT_WRITTEN, "workspace", "members", "list", "");
    expect(out.split("\n").slice(0, 3)).toEqual(["[workspace]", 'name = "shop"', 'default = "api"']);
  });

  it("does not count a bracket inside a string or a comment", () => {
    const raw = ["[workspace]", 'members = [ # a [ comment', '    "a]b",', "]", 'default = "x"'].join("\n");
    const out = patch(raw, "workspace", "members", "list", "c");
    expect(out).toBe(["[workspace]", 'members = ["c"]', 'default = "x"'].join("\n"));
  });

  it("does not count brackets inside a multi-line string, and never cuts past its array", () => {
    // Valid TOML (checked with tomllib) whose `]` and `[` sit inside `"""`/`'''`
    // strings. Scanning each line afresh deleted `default` and `name` on the first,
    // and left a dangling `[''', "y"]` on the second.
    const basic = ["[workspace]", 'members = ["""', 'api""", "web"]', 'default = "api"', 'name = """', ']shop"""'].join("\n");
    expect(patch(basic, "workspace", "members", "list", "api\nweb")).toBe(
      ["[workspace]", 'members = ["api", "web"]', 'default = "api"', 'name = """', ']shop"""'].join("\n"),
    );
    const literal = ["[workspace]", "members = ['''x", "[''', \"y\"]", 'default = "x"'].join("\n");
    expect(patch(literal, "workspace", "members", "list", "z")).toBe(
      ["[workspace]", 'members = ["z"]', 'default = "x"'].join("\n"),
    );
  });

  it("leaves a one-line array a one-line replacement", () => {
    const raw = ["[workspace]", 'members = ["a"]', 'default = "a"'].join("\n");
    expect(patch(raw, "workspace", "members", "list", "b")).toBe(
      ["[workspace]", 'members = ["b"]', 'default = "a"'].join("\n"),
    );
  });

  it("touches only the key's own line when an array is never closed", () => {
    const raw = ["[workspace]", "members = [", '"a",'].join("\n");
    expect(patch(raw, "workspace", "members", "list", "b")).toBe(
      ["[workspace]", 'members = ["b"]', '"a",'].join("\n"),
    );
  });
});

describe("dropEmptyTable — a table whose presence is a value (S-430, FR-UI-38)", () => {
  it("removes a header left holding no key, and nothing else", () => {
    const raw = ["[workspace]", 'name = "a"', "", "[workspace.autodiscover]", "", "[workspace.warm]", "concurrency = 2"].join("\n");
    expect(dropEmptyTable(raw, "workspace.autodiscover")).toBe(
      ["[workspace]", 'name = "a"', "", "", "[workspace.warm]", "concurrency = 2"].join("\n"),
    );
  });

  it("treats a comment as no key: the header goes, the operator's comment stays", () => {
    const raw = ["[workspace.autodiscover]", "# turned off while api/ is split", "[workspace.warm]"].join("\n");
    expect(dropEmptyTable(raw, "workspace.autodiscover")).toBe(["# turned off while api/ is split", "[workspace.warm]"].join("\n"));
  });

  it("keeps a table that still declares a key, and leaves an absent one alone", () => {
    const raw = ["[workspace.autodiscover]", "# why", "enabled = true"].join("\n");
    expect(dropEmptyTable(raw, "workspace.autodiscover")).toBe(raw);
    expect(dropEmptyTable('[workspace]\nname = "a"', "workspace.autodiscover")).toBe('[workspace]\nname = "a"');
  });
});
