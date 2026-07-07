/*
 * Unit tests for the Config editor's TOML line-patcher (S-191, FR-UI-12) — the
 * deterministic typed-field ⇄ raw-TOML round-trip. Pure string-in/string-out, so
 * no DOM is needed. Mirrors the legacy `config-editor.js` patch semantics: replace
 * in place, insert after an existing header, create an absent table, and remove a
 * cleared optional key.
 */

import { describe, expect, it } from "vitest";

import { patch, tomlValue } from "./toml.ts";

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
