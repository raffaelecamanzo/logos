import { beforeEach, describe, expect, it } from "vitest";

import type { ConfigReadModel } from "../../api/types.ts";
import type { SseFrame } from "../../api/sse.ts";
import {
  applyWikiFrame,
  effectiveWikiModel,
  hasWikiConsent,
  initialWikiGenState,
  isWikiConfigured,
  rememberWikiConsent,
  wikiDisclosure,
  WIKI_CONSENT_KEY,
} from "./wikiGenModel.ts";

/** Build an SSE frame the way the reader yields it. */
function frame(name: string, data: string): SseFrame {
  return { name, data };
}

/** A config read-model with the given chat/wiki model + key presence. */
function config(opts: {
  chatModel?: string | null;
  wikiModel?: string | null;
  provider?: "openai" | "anthropic";
  baseUrl?: string;
  keyPresent?: boolean;
}): ConfigReadModel {
  return {
    config: {
      path: ".logos/config.toml",
      exists: true,
      content: "",
      parsed: {
        languages: [],
        include: [],
        exclude: [],
        max_file_size: 1048576,
        framework_hints: [],
        chat: {
          provider: opts.provider ?? "openai",
          model: opts.chatModel ?? null,
          base_url: opts.baseUrl ?? "https://openrouter.ai/api/v1",
        },
        wiki: { model: opts.wikiModel ?? null },
      },
    },
    rules: { path: ".logos/rules.toml", exists: false, content: "", parsed: { constraints: {}, metric_thresholds: {} } },
    chat_key: { present: opts.keyPresent ?? true, last4: opts.keyPresent === false ? null : "9f3a" },
    defaults: DEFAULTS_FIXTURE,
  };
}

/** A minimal but shape-complete `defaults` projection (CR-067/BR-37) — this
 *  suite doesn't exercise default-rendering, so the values only need to match
 *  the wire shape, not the real server-computed numbers. */
const DEFAULTS_FIXTURE: ConfigReadModel["defaults"] = {
  config: {
    languages: [],
    include: ["**"],
    exclude: [],
    max_file_size: 2097152,
    framework_hints: [],
    chat: { provider: "openai", model: null, base_url: "https://openrouter.ai/api/v1" },
    wiki: {},
  },
  rules: {
    metric_thresholds: {
      nesting_depth: 4,
      brain_complexity: 15,
      brain_lines: 100,
      brain_nesting: 3,
      god_methods: 20,
      god_span: 500,
      clone_similarity: 0.85,
      clone_min_tokens: 50,
    },
    constraints: {},
  },
};

describe("applyWikiFrame — per-run reducer (S-178, FR-WK-18, NFR-CC-04)", () => {
  it("folds a full run: started → page-started → page-written → completed", () => {
    let s = initialWikiGenState();
    s = applyWikiFrame(s, frame("started", JSON.stringify({ total: 2 })));
    expect(s.phase).toBe("running");
    expect(s.total).toBe(2);

    s = applyWikiFrame(s, frame("page-started", JSON.stringify({ slug: "overview/x", title: "X", index: 1, total: 2 })));
    expect(s.current).toBe("overview/x");

    s = applyWikiFrame(s, frame("page-written", JSON.stringify({ slug: "overview/x", anchor_count: 1, replaced: true })));
    expect(s.written).toEqual(["overview/x"]);
    expect(s.current).toBeNull();

    s = applyWikiFrame(s, frame("completed", JSON.stringify({ pages_written: 1, pages_failed: 0 })));
    expect(s.phase).toBe("done");
    expect(s.current).toBeNull();
  });

  it("records a page failure honestly and keeps going", () => {
    let s = applyWikiFrame(initialWikiGenState(), frame("started", JSON.stringify({ total: 1 })));
    s = applyWikiFrame(s, frame("page-failed", JSON.stringify({ slug: "overview/y", error: "over-cap body" })));
    expect(s.failed).toEqual([{ slug: "overview/y", error: "over-cap body" }]);
    // A failure is not the terminal phase — a Completed still lands.
    expect(s.phase).toBe("running");
  });

  it("records an honest halt reason (budget/provider)", () => {
    let s = applyWikiFrame(initialWikiGenState(), frame("started", JSON.stringify({ total: 3 })));
    s = applyWikiFrame(s, frame("halted", JSON.stringify({ reason: "the per-run budget of 1 was spent" })));
    expect(s.halted).toContain("budget");
  });

  it("carries the configured per-page synthesis timeout from `started` (CR-059, S-239, FR-UI-24)", () => {
    const s = applyWikiFrame(
      initialWikiGenState(),
      frame("started", JSON.stringify({ total: 2, synthesis_timeout_secs: 180 })),
    );
    expect(s.synthesisTimeoutSecs).toBe(180);
  });

  it("defaults the synthesis timeout to null when `started` omits it (a malformed/older frame)", () => {
    const s = applyWikiFrame(initialWikiGenState(), frame("started", JSON.stringify({ total: 2 })));
    expect(s.synthesisTimeoutSecs).toBeNull();
  });

  it("treats configure-first / error / busy as plain-text terminal frames (not JSON)", () => {
    expect(applyWikiFrame(initialWikiGenState(), frame("configure-first", "choose a model in the Config tab")).phase).toBe(
      "configure-first",
    );
    expect(applyWikiFrame(initialWikiGenState(), frame("configure-first", "choose a model in the Config tab")).message).toContain(
      "Config",
    );
    expect(applyWikiFrame(initialWikiGenState(), frame("error", "wiki generation failed: boom")).phase).toBe("error");
    expect(applyWikiFrame(initialWikiGenState(), frame("busy", "")).phase).toBe("busy");
  });

  it("drops a malformed progress frame rather than guessing (NFR-CC-04)", () => {
    const before = applyWikiFrame(initialWikiGenState(), frame("started", JSON.stringify({ total: 1 })));
    const after = applyWikiFrame(before, frame("page-written", "not json"));
    expect(after).toEqual(before);
  });

  it("does not double-count a repeated page-written for the same slug", () => {
    let s = applyWikiFrame(initialWikiGenState(), frame("started", JSON.stringify({ total: 1 })));
    const written = frame("page-written", JSON.stringify({ slug: "a", anchor_count: 0, replaced: false }));
    s = applyWikiFrame(s, written);
    s = applyWikiFrame(s, written);
    expect(s.written).toEqual(["a"]);
  });
});

describe("configure-first + endpoint disclosure (FR-CF-07, NFR-SE-07)", () => {
  it("prefers the dedicated [wiki].model, else falls back to [chat].model", () => {
    expect(effectiveWikiModel(config({ wikiModel: "wiki/m", chatModel: "chat/m" }))).toBe("wiki/m");
    expect(effectiveWikiModel(config({ wikiModel: null, chatModel: "chat/m" }))).toBe("chat/m");
    expect(effectiveWikiModel(config({ wikiModel: null, chatModel: null }))).toBeNull();
    // A blank wiki model falls through to the chat model.
    expect(effectiveWikiModel(config({ wikiModel: "  ", chatModel: "chat/m" }))).toBe("chat/m");
  });

  it("is configured only with an effective model AND a present key", () => {
    expect(isWikiConfigured(config({ chatModel: "chat/m", keyPresent: true }))).toBe(true);
    expect(isWikiConfigured(config({ chatModel: "chat/m", keyPresent: false }))).toBe(false);
    expect(isWikiConfigured(config({ chatModel: null, keyPresent: true }))).toBe(false);
  });

  it("discloses the anthropic host for anthropic, else the base_url host — never the key", () => {
    const anth = wikiDisclosure(config({ provider: "anthropic", chatModel: "claude" }));
    expect(anth.endpointHost).toBe("api.anthropic.com");
    expect(anth.model).toBe("claude");

    const oai = wikiDisclosure(
      config({ provider: "openai", chatModel: "gpt", baseUrl: "https://openrouter.ai/api/v1" }),
    );
    expect(oai.endpointHost).toBe("openrouter.ai");
    // The disclosure carries no key material by construction.
    expect(JSON.stringify(oai)).not.toContain("9f3a");
  });
});

describe("consent gate (NFR-SE-07)", () => {
  beforeEach(() => window.localStorage.clear());

  it("defaults to no consent and remembers acceptance under the wiki-specific key", () => {
    expect(hasWikiConsent()).toBe(false);
    rememberWikiConsent();
    expect(hasWikiConsent()).toBe(true);
    expect(window.localStorage.getItem(WIKI_CONSENT_KEY)).toBe("1");
  });
});
