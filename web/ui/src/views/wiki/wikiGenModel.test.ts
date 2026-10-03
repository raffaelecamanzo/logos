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

/** A config read-model with the given chat/wiki model + key presence. The
 *  `effective_wiki` slice is the server's single-root resolution of the same
 *  document unless `effectiveWiki` states it (the server owns the rule —
 *  `WikiConfig::resolve_in_workspace`, pinned against the run by
 *  `the_read_model_slice_is_the_model_the_run_resolves`). */
function config(opts: {
  chatModel?: string | null;
  wikiModel?: string | null;
  effectiveWiki?: string | null;
  provider?: "openai" | "anthropic";
  baseUrl?: string;
  keyPresent?: boolean;
}): ConfigReadModel {
  const singleRootWiki = opts.wikiModel?.trim() || opts.chatModel || null;
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
    // The single-root slice for the same document (S-448): each half is the
    // member's own when declared, else unset.
    effective_chat: {
      policy: {
        provider: opts.provider ?? "openai",
        model: opts.chatModel ?? null,
        base_url: opts.baseUrl ?? "https://openrouter.ai/api/v1",
      },
      policy_origin: opts.chatModel ? "member" : "unset",
      credential: opts.keyPresent === false ? { present: false } : { present: true, last4: "9f3a" },
      credential_origin: opts.keyPresent === false ? "unset" : "member",
      member_key_withheld: false,
    },
    effective_wiki: { model: opts.effectiveWiki === undefined ? singleRootWiki : opts.effectiveWiki },
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
      duplicate_min_tokens: 50,
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

describe("applyWikiFrame — live-scope denominator (CR-093, S-310, FR-UI-19, NFR-CC-04)", () => {
  const started = (total: number) => frame("started", JSON.stringify({ total, synthesis_timeout_secs: 180 }));
  const pageStarted = (slug: string, index: number, total: number) =>
    frame("page-started", JSON.stringify({ slug, title: slug, index, total }));
  const pageWritten = (slug: string) =>
    frame("page-written", JSON.stringify({ slug, anchor_count: 0, replaced: false }));
  const fold = (frames: SseFrame[], from = initialWikiGenState()) => frames.reduce(applyWikiFrame, from);

  /** A budget-1 run that opened on two pages (a, b): the re-read after page a
   *  surfaced b, c and d (1 attempted + 3 surfaced = 4); the re-read after page b
   *  found d gone from the work-list unattempted (2 + 1 = 3). */
  const GROWN_RUN: SseFrame[] = [
    started(2),
    pageStarted("a", 1, 2),
    pageWritten("a"),
    pageStarted("b", 2, 4),
    pageWritten("b"),
    pageStarted("c", 3, 3),
    pageWritten("c"),
    frame("completed", JSON.stringify({ pages_written: 3, pages_failed: 0 })),
  ];

  it("adopts the per-page total as a monotonic maximum, keeping the opening size", () => {
    let s = fold([started(5), pageStarted("a", 1, 5)]);
    expect(s.total).toBe(5);
    expect(s.initialTotal).toBe(5);

    s = applyWikiFrame(s, pageStarted("b", 2, 7));
    expect(s.total).toBe(7);

    // An item left the work-list unattempted: the agent's total shrank, the
    // rendered denominator does not walk backwards.
    s = applyWikiFrame(s, pageStarted("c", 3, 6));
    expect(s.total).toBe(7);
    expect(s.initialTotal).toBe(5);
  });

  it("never renders a numerator above its denominator on a run whose scope grew", () => {
    let s = initialWikiGenState();
    for (const f of GROWN_RUN) {
      s = applyWikiFrame(s, f);
      expect(s.written.length).toBeLessThanOrEqual(s.total);
    }
    expect(s.total).toBe(4);
    expect(s.written).toEqual(["a", "b", "c"]);
  });

  it("converges a mid-run re-attach on the same denominator, at every join point", () => {
    const live = fold(GROWN_RUN);
    for (let k = 1; k <= GROWN_RUN.length; k++) {
      // A re-attach at frame k: a FRESH state (a remounted hook) replays the retained
      // history, the boundary frame arriving twice at the replay/live handoff, then
      // the live tail — the fold is idempotent, so it lands where the live one did.
      const replayed = fold([...GROWN_RUN.slice(0, k), GROWN_RUN[k - 1], ...GROWN_RUN.slice(k)]);
      expect({ total: replayed.total, initial: replayed.initialTotal, written: replayed.written }).toEqual({
        total: live.total,
        initial: live.initialTotal,
        written: live.written,
      });
    }
  });

  it("leaves an unchanged-scope run's denominator exactly at the started size", () => {
    const s = fold([started(2), pageStarted("a", 1, 2), pageWritten("a"), pageStarted("b", 2, 2), pageWritten("b")]);
    expect(s.total).toBe(2);
    expect(s.initialTotal).toBe(2);
  });
});

describe("configure-first + endpoint disclosure (FR-CF-07, NFR-SE-07)", () => {
  it("is the server's effective_wiki model, never re-derived from the literal document", () => {
    // The literal document and the chat slice would say "chat/m"; the server's
    // resolution says otherwise, and the server's resolution is what the run uses.
    const c = config({ wikiModel: null, chatModel: "chat/m", effectiveWiki: "resolved/m" });
    expect(effectiveWikiModel(c)).toBe("resolved/m");
    expect(effectiveWikiModel(config({ wikiModel: "wiki/m", chatModel: "chat/m", effectiveWiki: null }))).toBeNull();
    // A blank slice is no model (configure-first), not an empty-string model.
    expect(effectiveWikiModel(config({ chatModel: "chat/m", effectiveWiki: "  " }))).toBeNull();
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

  // Workspace inheritance (FR-WS-30, ADR-67): the member's literal document declares
  // no [chat] and holds no key, but the effective slice resolves both from the
  // workspace root. The server generates for this member (S-449), so the tab must
  // not show configure-first, and must disclose the INHERITED endpoint.
  function inherited(): ConfigReadModel {
    const c = config({ keyPresent: false });
    c.effective_chat = {
      policy: { provider: "openai", model: "ws-model", base_url: "https://llm.example.org/v1" },
      policy_origin: "workspace",
      credential: { present: true, last4: "KEY1" },
      credential_origin: "workspace",
      member_key_withheld: false,
    };
    // The server's resolution for this state: no wiki model anywhere, so the
    // effective (inherited) chat model.
    c.effective_wiki = { model: "ws-model" };
    return c;
  }

  it("is configured from an inherited workspace policy and key (FR-WS-30)", () => {
    const c = inherited();
    expect(c.config.parsed.chat.model).toBeNull();
    expect(c.chat_key.present).toBe(false);
    expect(effectiveWikiModel(c)).toBe("ws-model");
    expect(isWikiConfigured(c)).toBe(true);
  });

  it("the member's own [wiki].model still wins over the inherited chat model", () => {
    const c = inherited();
    c.config.parsed.wiki = { model: "member-wiki-model" };
    c.effective_wiki = { model: "member-wiki-model" };
    expect(effectiveWikiModel(c)).toBe("member-wiki-model");
  });

  // Sprint 77 HF-1, server↔SPA agreement: a member inheriting the chat policy
  // inherits the workspace [wiki].model, which appears in NEITHER the member's
  // literal document NOR the chat slice — only in the server's effective_wiki
  // slice. The tab's model, readiness and disclosure must name it, as the run does.
  it("names the inherited workspace [wiki].model the run uses (HF-1)", () => {
    const c = inherited();
    c.effective_wiki = { model: "ws-wiki-model" };
    expect(c.config.parsed.wiki?.model ?? null).toBeNull();
    expect(c.effective_chat.policy.model).toBe("ws-model");
    expect(effectiveWikiModel(c)).toBe("ws-wiki-model");
    expect(isWikiConfigured(c)).toBe(true);
    const d = wikiDisclosure(c);
    expect(d.model).toBe("ws-wiki-model");
    expect(d.endpointHost).toBe("llm.example.org");
  });

  it("with no effective wiki model the tab is configure-first even with a key", () => {
    const c = inherited();
    c.effective_wiki = { model: null };
    expect(isWikiConfigured(c)).toBe(false);
    expect(wikiDisclosure(c).model).toBe("(no model)");
  });

  it("discloses the inherited endpoint host, never the member literal's", () => {
    const d = wikiDisclosure(inherited());
    expect(d.endpointHost).toBe("llm.example.org");
    expect(d.model).toBe("ws-model");
    expect(JSON.stringify(d)).not.toContain("KEY1");
  });

  it("an inherited policy with no key anywhere stays configure-first", () => {
    const c = inherited();
    c.effective_chat.credential = { present: false };
    c.effective_chat.credential_origin = "unset";
    expect(isWikiConfigured(c)).toBe(false);
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
