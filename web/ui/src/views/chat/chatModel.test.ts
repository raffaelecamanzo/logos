import { afterEach, describe, expect, it } from "vitest";

import {
  ANTHROPIC_HOST,
  applyFrame,
  boundNote,
  chatStorageKey,
  consentEntries,
  CONSENT_KEY,
  memberChatStorageScope,
  memberReadRootsDisclosure,
  READ_ROOTS_CONSENT_KEY,
  endpointHost,
  hasConsent,
  hostOf,
  chatReadiness,
  chatScope,
  configureFirstCopy,
  initialTurn,
  modelLabel,
  readRoots,
  parseSseBlock,
  readSseStream,
  rememberConsent,
  roleLabel,
  turnEndedEmpty,
  workspaceChatReadiness,
  workspaceConfigRepairHref,
  workspaceConfigureFirstCopy,
  workspaceReadRootsDisclosure,
  WORKSPACE_CHAT_SCOPE,
  WORKSPACE_CONFIG_FILE,
  WORKSPACE_SECRETS_FILE,
  type ChatConfigReadModel,
  type ChatOrigin,
  type ChatPolicy,
  type ChatReady,
  type ChatScope,
  type MemberChatReadRoots,
  type SseFrame,
  type WorkspaceChatConfigReadModel,
  type TurnState,
  WORKSPACE_CONFIG_HREF,
} from "./chatModel.ts";
import { WORKSPACE_NAV_ITEMS } from "../../nav.ts";

const POLICY: ChatPolicy = {
  provider: "openai",
  model: "openrouter/some-model",
  base_url: "https://openrouter.ai/api/v1",
  max_tool_calls: 24,
  max_subagent_tool_calls: 8,
  max_replans: 3,
};

/** The effective-chat read-model slice (S-448) with the two origins the readiness
 *  verdict is read off. `policy` is the effective table; a half the resolution
 *  found no declaration for carries `unset`. */
function configModel(
  policyOrigin: ChatOrigin,
  credentialOrigin: ChatOrigin,
  memberKeyWithheld = false,
): ChatConfigReadModel {
  return {
    effective_chat: {
      policy: policyOrigin === "unset" ? { ...POLICY, model: null } : POLICY,
      policy_origin: policyOrigin,
      credential: { present: credentialOrigin !== "unset" },
      credential_origin: credentialOrigin,
      member_key_withheld: memberKeyWithheld,
    },
  };
}

/** Build a byte ReadableStream from string chunks (an SSE wire fixture). */
function streamOf(chunks: string[]): ReadableStream<Uint8Array> {
  const enc = new TextEncoder();
  return new ReadableStream({
    start(controller) {
      for (const c of chunks) controller.enqueue(enc.encode(c));
      controller.close();
    },
  });
}

/** Fold a list of frames into a turn (the reducer over a sequence). */
function fold(frames: SseFrame[]): TurnState {
  return frames.reduce(applyFrame, initialTurn());
}

describe("parseSseBlock", () => {
  it("parses the event name and data payload", () => {
    expect(parseSseBlock("event: plan\ndata: {\"round\":0}")).toEqual({
      name: "plan",
      data: '{"round":0}',
    });
  });

  it("defaults the event name to `message` and joins multi-line data", () => {
    expect(parseSseBlock("data: a\ndata: b")).toEqual({ name: "message", data: "a\nb" });
  });

  it("drops a keep-alive comment / data-less block", () => {
    expect(parseSseBlock(": keep-alive")).toBeNull();
    expect(parseSseBlock("event: plan")).toBeNull();
  });
});

describe("applyFrame — incremental turn rendering", () => {
  it("records the plan and a revised plan supersedes it", () => {
    const t = fold([
      { name: "plan", data: '{"round":0,"steps":[{"role":"graph_navigator","instruction":"map callers"}]}' },
      { name: "plan", data: '{"round":1,"steps":[{"role":"source_reader","instruction":"read file"}]}' },
    ]);
    expect(t.plan?.round).toBe(1);
    expect(t.plan?.steps).toHaveLength(1);
    expect(t.plan?.steps[0].role).toBe("source_reader");
  });

  it("starts an activity chip and marks it done with its summary on observe", () => {
    const t = fold([
      { name: "step_started", data: '{"index":0,"role":"graph_navigator","instruction":"map callers"}' },
      { name: "step_observed", data: '{"index":0,"role":"graph_navigator","summary":"found 3 callers"}' },
    ]);
    expect(t.chips).toHaveLength(1);
    expect(t.chips[0]).toMatchObject({ index: 0, role: "graph_navigator", done: true, summary: "found 3 callers" });
  });

  it("stamps each chip with its round and matches step_observed on the (round, index) PAIR, not index alone (S-303, CR-090)", () => {
    // The orchestrator restarts `index` at 0 every replan round: round 1's step 0
    // collides with round 0's step 0. A round-blind reducer would let round 1's
    // observation mark BOTH chips done, or attribute round 1's summary to round
    // 0's chip.
    const t = fold([
      { name: "plan", data: '{"round":0,"steps":[{"role":"graph_navigator","instruction":"map callers"}]}' },
      { name: "step_started", data: '{"index":0,"role":"graph_navigator","instruction":"map callers"}' },
      { name: "plan", data: '{"round":1,"steps":[{"role":"source_reader","instruction":"read file"}]}' },
      { name: "step_started", data: '{"index":0,"role":"source_reader","instruction":"read file"}' },
      { name: "step_observed", data: '{"index":0,"role":"source_reader","summary":"round 1 result"}' },
    ]);

    expect(t.chips).toHaveLength(2);
    const [roundZeroChip, roundOneChip] = t.chips;

    // Round 0's step never ran to observation — it must stay running, with no
    // summary borrowed from round 1's observation of the SAME index.
    expect(roundZeroChip).toMatchObject({ round: 0, index: 0, done: false });
    expect(roundZeroChip.summary).toBeUndefined();
    // Round 1's step is the one actually observed, and carries its OWN summary.
    expect(roundOneChip).toMatchObject({ round: 1, index: 0, done: true, summary: "round 1 result" });

    // Grouping source: every round's plan is retained, not just the latest.
    expect(t.plans).toEqual([
      { round: 0, steps: [{ role: "graph_navigator", instruction: "map callers" }] },
      { round: 1, steps: [{ role: "source_reader", instruction: "read file" }] },
    ]);
  });

  it("streams answer deltas then reconciles to the authoritative final answer", () => {
    const t = fold([
      { name: "answer_delta", data: '{"delta":"Hel"}' },
      { name: "answer_delta", data: '{"delta":"lo"}' },
      { name: "final_answer", data: '{"answer":"Hello, world."}' },
    ]);
    expect(t.answer).toBe("Hello, world.");
    expect(t.streaming).toBe(false);
    expect(t.finalized).toBe(true);
  });

  it("keeps streamed text when final_answer carries no body", () => {
    const t = fold([
      { name: "answer_delta", data: '{"delta":"partial"}' },
      { name: "final_answer", data: "{}" },
    ]);
    expect(t.answer).toBe("partial");
    expect(t.streaming).toBe(false);
  });

  it("renders an honest halt note (never a fabricated answer)", () => {
    const t = fold([
      { name: "step_started", data: '{"index":0,"role":"source_reader","instruction":"read"}' },
      { name: "halted", data: '{"round":1,"bound":{"bound":"global_tool_calls","limit":24}}' },
    ]);
    expect(t.halt).toBe("halted: the global per-turn tool-call ceiling was reached (24 calls)");
    expect(t.answer).toBe("");
  });

  it("captures an honest error from a plain-text error frame", () => {
    const t = fold([{ name: "error", data: "provider request failed: 503" }]);
    expect(t.error).toBe("provider request failed: 503");
  });

  it("drops a malformed (non-error) frame rather than guessing", () => {
    const t = applyFrame(initialTurn(), { name: "plan", data: "not json {" });
    expect(t).toEqual(initialTurn());
  });

  it("guards a non-array plan.steps so a malformed frame cannot crash the render", () => {
    const t = applyFrame(initialTurn(), { name: "plan", data: '{"round":0,"steps":"oops"}' });
    expect(t.plan).toEqual({ round: 0, steps: [] });
  });
});

describe("readSseStream", () => {
  it("reads a full turn from a byte stream (plan → activity → answer → final)", async () => {
    const frames: SseFrame[] = [];
    await readSseStream(
      streamOf([
        'event: plan\ndata: {"round":0,"steps":[]}\n\n',
        'event: step_started\ndata: {"index":0,"role":"graph_navigator","instruction":"x"}\n\n',
        // a chunk boundary in the middle of a block is reassembled
        'event: answer_de',
        'lta\ndata: {"delta":"hi"}\n\nevent: final_answer\ndata: {"answer":"hi"}\n\n',
      ]),
      (f) => frames.push(f),
    );
    expect(frames.map((f) => f.name)).toEqual(["plan", "step_started", "answer_delta", "final_answer"]);
    expect(fold(frames).answer).toBe("hi");
  });

  it("surfaces a halted branch from the stream", async () => {
    const frames: SseFrame[] = [];
    await readSseStream(
      streamOf(['event: halted\ndata: {"round":1,"bound":{"bound":"replans","limit":3}}\n\n']),
      (f) => frames.push(f),
    );
    expect(fold(frames).halt).toBe("halted: the planner reached the max-replans bound (3 replans)");
  });

  it("surfaces an error branch from the stream and flushes a trailing block", async () => {
    const frames: SseFrame[] = [];
    // No trailing blank line — exercises the tail flush.
    await readSseStream(streamOf(["event: error\ndata: boom"]), (f) => frames.push(f));
    expect(fold(frames).error).toBe("boom");
  });

  it("is a no-op for a null body", async () => {
    let called = 0;
    await readSseStream(null, () => (called += 1));
    expect(called).toBe(0);
  });
});

describe("display helpers", () => {
  it("labels every subagent role and falls back for an unknown one", () => {
    expect(roleLabel("graph_navigator")).toBe("Graph-Navigator");
    expect(roleLabel("governance_analyst")).toBe("Governance-Analyst");
    expect(roleLabel("source_reader")).toBe("Source-Reader");
    expect(roleLabel("synthesizer")).toBe("Synthesizer");
    expect(roleLabel("mystery")).toBe("mystery");
  });

  it("names each budget bound honestly", () => {
    expect(boundNote({ bound: "global_tool_calls", limit: 24 })).toContain("global per-turn tool-call ceiling");
    expect(boundNote({ bound: "subagent_tool_calls", limit: 8 })).toContain("per-subagent tool-call cap");
    expect(boundNote({ bound: "replans", limit: 3 })).toContain("max-replans");
    expect(boundNote(undefined)).toBe("the turn halted at a budget bound");
  });
});

describe("endpoint disclosure", () => {
  it("extracts a host authority and falls back for a scheme-less URL", () => {
    expect(hostOf("https://openrouter.ai/api/v1")).toBe("openrouter.ai");
    expect(hostOf("http://localhost:8080/v1")).toBe("localhost:8080");
    expect(hostOf("openrouter.ai")).toBe("openrouter.ai");
  });

  it("names the native Anthropic host for the anthropic provider", () => {
    expect(endpointHost({ ...POLICY, provider: "anthropic" })).toBe(ANTHROPIC_HOST);
    expect(endpointHost(POLICY)).toBe("openrouter.ai");
  });
});

describe("modelLabel (S-309, NFR-CC-04) — the declared model, or an honest note", () => {
  it("names the declared model verbatim", () => {
    expect(modelLabel(POLICY)).toBe("openrouter/some-model");
  });

  it("states an absent model honestly rather than a fabricated default", () => {
    // `resolve_chat` only resolves a policy origin once its model is declared
    // (`logos-core/src/config/chat.rs`), so this combination is not reachable via
    // the server today — the type stays optional, and this pins the honest
    // fallback for the day it changes rather than a silently-printed `undefined`.
    expect(modelLabel({ ...POLICY, model: null })).toBe("no model configured");
    expect(modelLabel({ ...POLICY, model: undefined })).toBe("no model configured");
    expect(modelLabel({ ...POLICY, model: "" })).toBe("no model configured");
    expect(modelLabel({ ...POLICY, model: "   " })).toBe("no model configured");
  });
});

describe("chatReadiness (S-452, FR-UI-18) — the full origin matrix, no DOM", () => {
  const SINGLE: ChatScope = { mode: "single" };
  const WORKSPACE: ChatScope = { mode: "workspace", member: "billing-service" };

  // The origin pairs S-447's resolution matrix produces, named by the shape that
  // yields each, with HF-1's withheld flag. `workspace` policy + `member` key is not
  // among them: an inherited policy takes the workspace key only (ADR-67 §2), so the
  // member-key-only shapes under a workspace policy appear as withheld rows. A
  // single-root payload never carries `workspace`, but the verdict must not depend
  // on the mode for readiness, so every row runs in both.
  const MATRIX: {
    shape: string;
    policy: ChatOrigin;
    credential: ChatOrigin;
    withheld?: boolean;
    ready: boolean;
    absent?: "model" | "key" | "both";
    present?: { half: "model" | "key"; origin: "member" | "workspace" } | null;
  }[] = [
    { shape: "member declares both", policy: "member", credential: "member", ready: true },
    { shape: "workspace declares both", policy: "workspace", credential: "workspace", ready: true },
    { shape: "member policy, workspace key", policy: "member", credential: "workspace", ready: true },
    { shape: "member key only, workspace declares both", policy: "workspace", credential: "workspace", withheld: true, ready: true },
    { shape: "nothing anywhere", policy: "unset", credential: "unset", ready: false, absent: "both", present: null },
    { shape: "member key only", policy: "unset", credential: "member", ready: false, absent: "model", present: { half: "key", origin: "member" } },
    { shape: "workspace key only", policy: "unset", credential: "workspace", ready: false, absent: "model", present: { half: "key", origin: "workspace" } },
    { shape: "member model only", policy: "member", credential: "unset", ready: false, absent: "key", present: { half: "model", origin: "member" } },
    { shape: "workspace model only", policy: "workspace", credential: "unset", ready: false, absent: "key", present: { half: "model", origin: "workspace" } },
    { shape: "member key only, workspace declares only the model", policy: "workspace", credential: "unset", withheld: true, ready: false, absent: "key", present: { half: "model", origin: "workspace" } },
  ];

  for (const scope of [SINGLE, WORKSPACE]) {
    for (const row of MATRIX) {
      it(`${scope.mode}: ${row.shape} → ${row.ready ? "ready" : `configure-first (${row.absent})`}`, () => {
        const verdict = chatReadiness(configModel(row.policy, row.credential, row.withheld), scope);
        expect(verdict.ready).toBe(row.ready);
        if (verdict.ready) {
          // The configured surface receives the EFFECTIVE policy, not the member literal.
          expect(verdict.policy).toEqual(POLICY);
          expect(verdict.policyOrigin).toBe(row.policy);
        } else {
          expect(verdict.absent).toBe(row.absent);
          expect(verdict.present).toEqual(row.present);
          expect(verdict.memberKeyWithheld).toBe(row.withheld ?? false);
        }
      });
    }
  }

  it("reads the verdict off the origins, never off the policy's model", () => {
    // A slice whose policy still carries a model but whose origin is unset (the seam
    // treats a blank model as undeclared) is configure-first — the tab and the turn
    // path read the same two origins (ADR-67), not a second model check.
    const m = configModel("unset", "member");
    m.effective_chat.policy = POLICY;
    expect(chatReadiness(m, SINGLE).ready).toBe(false);
  });

  it("names 'this repository' in single-root mode and never a member", () => {
    const v = chatReadiness(configModel("unset", "unset"), SINGLE);
    if (v.ready) throw new Error("expected configure-first");
    expect(v.root).toEqual({ kind: "repository", label: "this repository" });
    expect(v.configHref).toBe("/config");
    expect(v.workspaceFiles).toEqual([]);
  });

  it("names the member by name in workspace mode and links ITS Config tab", () => {
    const v = chatReadiness(configModel("unset", "unset"), WORKSPACE);
    if (v.ready) throw new Error("expected configure-first");
    expect(v.root).toEqual({ kind: "member", label: "billing-service" });
    // A plain link reloads the shell, which re-opens whatever member the URL names —
    // so it must name this one, or it opens the default member's editor instead.
    expect(v.configHref).toBe("/config?repo=billing-service");
  });

  it("never fabricates a member name when the workspace selected none", () => {
    const v = chatReadiness(configModel("unset", "unset"), { mode: "workspace", member: null });
    if (v.ready) throw new Error("expected configure-first");
    expect(v.root.kind).toBe("default-member");
    expect(v.root.label).toBe("the workspace's default member");
    expect(v.configHref).toBe("/config");
  });

  it("names, as text, the workspace file each absent half would be declared in", () => {
    const files = (p: ChatOrigin, c: ChatOrigin) => {
      const v = chatReadiness(configModel(p, c), WORKSPACE);
      return v.ready ? null : v.workspaceFiles;
    };
    expect(files("unset", "unset")).toEqual([
      "<workspace-root>/.logos/config.toml",
      "<workspace-root>/.logos/secrets.toml",
    ]);
    expect(files("unset", "member")).toEqual(["<workspace-root>/.logos/config.toml"]);
    expect(files("member", "unset")).toEqual(["<workspace-root>/.logos/secrets.toml"]);
  });

  it("links the workspace Config view in workspace mode, keeping the member on the URL (S-451)", () => {
    const v = chatReadiness(configModel("unset", "unset"), WORKSPACE);
    if (v.ready) throw new Error("expected configure-first");
    // Its own member, so returning from the editor reopens this member's chat.
    expect(v.workspaceConfigHref).toBe("/workspace-config?repo=billing-service");
    const none = chatReadiness(configModel("unset", "unset"), { mode: "workspace", member: null });
    expect(none.ready ? null : none.workspaceConfigHref).toBe("/workspace-config");
  });

  it("links nothing at the workspace tier in single-root mode, where it does not exist", () => {
    for (const [p, c] of [["unset", "unset"], ["unset", "member"], ["member", "unset"]] as const) {
      const v = chatReadiness(configModel(p, c), SINGLE);
      expect(v.ready ? "ready" : v.workspaceConfigHref).toBeNull();
    }
  });

  it("reads the workspace Config route off its nav registration, never a second spelling", () => {
    const registered = WORKSPACE_NAV_ITEMS.find((i) => i.id === "workspace-config");
    expect(registered?.scope).toBe("app");
    expect(WORKSPACE_CONFIG_HREF).toBe(registered?.path);
  });
});

describe("configureFirstCopy (S-452) — the rendered claim, composed off the verdict", () => {
  const copy = (p: ChatOrigin, c: ChatOrigin, scope: ChatScope, withheld = false) => {
    const v = chatReadiness(configModel(p, c, withheld), scope);
    if (v.ready) throw new Error("expected configure-first");
    return configureFirstCopy(v);
  };
  const WS: ChatScope = { mode: "workspace", member: "billing-service" };

  it("single-root: names this repository, the absent half, and nothing about a workspace", () => {
    const c = copy("unset", "member", { mode: "single" });
    expect(c.summary).toBe("Chat is not configured yet for this repository — no provider model is declared.");
    expect(c.present).toBe("The API key is declared by this repository.");
    expect(c.action).toBe("Choose a provider model");
    expect(c.actionScope).toBe("");
    expect(c.workspaceLead).toBeNull();
    expect(Object.values(c).join(" ")).not.toMatch(/workspace/);
  });

  it("workspace: names the member, both roots looked in, and an inherited present half", () => {
    const c = copy("unset", "workspace", WS);
    expect(c.summary).toBe(
      "Chat is not configured yet for billing-service — no provider model is declared by billing-service or by the workspace root.",
    );
    expect(c.present).toBe("The API key is inherited from the workspace root.");
    expect(c.actionScope).toBe(" for billing-service");
    expect(c.workspaceLead).toBe("Or declare it once for every member of the workspace, in");
  });

  it("workspace: a member-declared present half is attributed to the member by name", () => {
    const c = copy("member", "unset", WS);
    expect(c.summary).toMatch(/— no API key is declared by billing-service or by the workspace root\.$/);
    expect(c.present).toBe("The provider model is declared by billing-service.");
    expect(c.action).toBe("Add an API key");
  });

  it("both absent: one sentence names both halves and no present origin is invented", () => {
    const c = copy("unset", "unset", WS);
    expect(c.summary).toMatch(/neither a provider model nor an API key is declared/);
    expect(c.present).toBeNull();
    expect(c.action).toBe("Choose a provider model and add an API key");
    expect(c.workspaceLead).toMatch(/^Or declare them once/);
  });

  it("names a withheld member key, and makes the member's own model the action (HF-1)", () => {
    const c = copy("workspace", "unset", WS, true);
    // Only the workspace root can supply a key for its endpoint, so only it is named.
    expect(c.summary).toBe(
      "Chat is not configured yet for billing-service — no API key is declared by the workspace root.",
    );
    expect(c.present).toBe("The provider model is inherited from the workspace root.");
    expect(c.memberKeyNote).toBe(
      "The API key billing-service declares is not used with the inherited workspace endpoint — setting a [chat] model on billing-service makes it use its own key.",
    );
    expect(c.action).toBe("Choose a provider model");
    // The alternative names what goes in the workspace file — a key, not the model
    // the action just named.
    expect(c.workspaceLead).toBe("Or declare an API key once for every member of the workspace, in");
  });

  it("under an inherited policy with no key anywhere, never advises a member key alone (HF-1)", () => {
    // No member key to withhold, but one added to the member would be withheld too:
    // only the workspace root is named, and the member route needs its own model.
    const c = copy("workspace", "unset", WS);
    expect(c.summary).toBe(
      "Chat is not configured yet for billing-service — no API key is declared by the workspace root.",
    );
    expect(c.memberKeyNote).toBe(
      "An API key added to billing-service is not used with the inherited workspace endpoint — it is used once billing-service declares its own [chat] model.",
    );
    expect(c.action).toBe("Choose a provider model and add an API key");
    expect(c.workspaceLead).toBe("Or declare an API key once for every member of the workspace, in");

    // A member-owned policy keeps the member-first copy and no note.
    const own = copy("member", "unset", WS);
    expect(own.memberKeyNote).toBeNull();
    expect(own.summary).toMatch(/no API key is declared by billing-service or by the workspace root\.$/);
    expect(own.action).toBe("Add an API key");
    expect(own.workspaceLead).toBe("Or declare it once for every member of the workspace, in");
  });

  it("refers to an unnamed default member's withheld key without a name", () => {
    const c = copy("workspace", "unset", { mode: "workspace", member: null }, true);
    expect(c.memberKeyNote).toBe(
      "The API key that member declares is not used with the inherited workspace endpoint — setting a [chat] model on that member makes it use its own key.",
    );
  });

  it("an unselected workspace member is referred to without a name", () => {
    const c = copy("member", "unset", { mode: "workspace", member: null });
    expect(c.summary).toMatch(/^Chat is not configured yet for the workspace's default member — /);
    expect(c.present).toBe("The provider model is declared by that member.");
    expect(c.actionScope).toBe("");
  });
});

describe("chatScope", () => {
  it("is a workspace scope only in workspace mode", () => {
    expect(chatScope("workspace", "api")).toEqual({ mode: "workspace", member: "api" });
    expect(chatScope("single", null)).toEqual({ mode: "single" });
    expect(chatScope("loading", null)).toEqual({ mode: "single" });
  });
});

describe("workspaceConfigRepairHref (HF-2, Sprint 77 review option 4i)", () => {
  it("single-root: no workspace tier, no link", () => {
    expect(workspaceConfigRepairHref({ mode: "single" })).toBeNull();
  });

  it("workspace: the workspace Config view, carrying the selected member", () => {
    expect(workspaceConfigRepairHref({ mode: "workspace", member: "billing-service" })).toBe(
      "/workspace-config?repo=billing-service",
    );
  });

  it("workspace with no member selected: the bare route, exactly as chatReadiness's own link", () => {
    expect(workspaceConfigRepairHref({ mode: "workspace", member: null })).toBe("/workspace-config");
  });
});

describe("turnEndedEmpty", () => {
  it("is true only for a turn with no answer, halt, or error", () => {
    expect(turnEndedEmpty(initialTurn())).toBe(true);
    expect(turnEndedEmpty({ ...initialTurn(), answer: "x" })).toBe(false);
    expect(turnEndedEmpty({ ...initialTurn(), halt: "h" })).toBe(false);
    expect(turnEndedEmpty({ ...initialTurn(), error: "e" })).toBe(false);
  });
});

describe("consent gate", () => {
  afterEach(() => window.localStorage.clear());

  it("remembers an acknowledgement across reads, under its scope's key", () => {
    expect(hasConsent("single")).toBe(false);
    rememberConsent("single");
    expect(window.localStorage.getItem(chatStorageKey(CONSENT_KEY, "single"))).toBe("1");
    expect(window.localStorage.getItem(CONSENT_KEY)).toBeNull();
    expect(hasConsent("single")).toBe(true);
  });

  it("covers declared read roots only for the exact set it disclosed (HF-1)", () => {
    // A plain first-use consent — given before any read root was declared —
    // still covers a policy with none, but not one that declares some.
    rememberConsent("single");
    expect(hasConsent("single", [])).toBe(true);
    expect(hasConsent("single", ["../logos-docs"])).toBe(false);

    rememberConsent("single", ["../logos-docs", "/srv/specs"]);
    expect(window.localStorage.getItem(chatStorageKey(READ_ROOTS_CONSENT_KEY, "single"))).toBe(
      '["../logos-docs","/srv/specs"]',
    );
    // Order and repetition do not matter; the SET does.
    expect(hasConsent("single", ["/srv/specs", "../logos-docs", "/srv/specs"])).toBe(true);
    expect(hasConsent("single", ["../logos-docs"])).toBe(false);
    expect(hasConsent("single", ["../logos-docs", "/srv/specs", "/extra"])).toBe(false);
  });

  it("never lets one scope's consent cover another's (S-485)", () => {
    rememberConsent("member:api");
    expect(hasConsent("member:api")).toBe(true);
    for (const other of ["member:web", "single", WORKSPACE_CHAT_SCOPE] as const) {
      expect(hasConsent(other)).toBe(false);
    }
  });

  it("does not trust the member-blind key every chat shared before S-485", () => {
    // Its scope is unknown — a workspace serve's member chat wrote it — so it is
    // fail-safe ignored: one fresh prompt, never a borrowed consent.
    window.localStorage.setItem(CONSENT_KEY, "1");
    expect(hasConsent("single")).toBe(false);
    expect(hasConsent(WORKSPACE_CHAT_SCOPE)).toBe(false);
  });
});

describe("the chat storage scope (S-485)", () => {
  it("spells the member chat's scope as the shell spells its cache key", () => {
    expect(memberChatStorageScope({ mode: "single" })).toBe("single");
    expect(memberChatStorageScope({ mode: "workspace", member: "api" })).toBe("member:api");
    expect(WORKSPACE_CHAT_SCOPE).toBe("workspace");
  });

  it("qualifies a key base by scope, so two scopes never share a key", () => {
    expect(chatStorageKey("logos.chat.activeThread", "member:api")).toBe(
      "logos.chat.activeThread:member:api",
    );
    expect(chatStorageKey("k", "member:api")).not.toBe(chatStorageKey("k", "member:web"));
    expect(chatStorageKey("k", "single")).not.toBe(chatStorageKey("k", "workspace"));
  });
});

const READY: ChatReady = { ready: true, policy: POLICY, policyOrigin: "member", credentialOrigin: "member" };

describe("the read-roots disclosure (sprint-79 HF-1, S-485)", () => {
  it("member chat: the policy's own roots, qualified only when inherited", () => {
    expect(memberReadRootsDisclosure(READY).groups).toEqual([]);
    const own = memberReadRootsDisclosure({ ...READY, policy: { ...POLICY, read_roots: ["../d"] } });
    expect(own.groups).toEqual([{ roots: ["../d"], relativeTo: null, readBy: [] }]);
    // Bare entries — the consent identity the member chat always had.
    expect(consentEntries(own)).toEqual(["../d"]);
    const inherited = memberReadRootsDisclosure({
      ...READY,
      policyOrigin: "workspace",
      policy: { ...POLICY, read_roots: ["../d"] },
    });
    expect(inherited.groups[0].relativeTo).toBe("the workspace root");
  });

  it("workspace chat: inherited roots once with their readers, owned roots per member, unreadable named", () => {
    const members: MemberChatReadRoots[] = [
      { name: "api", policy_origin: "member", declared_by: "member", read_roots: ["../api-docs"] },
      { name: "web", policy_origin: "workspace", declared_by: "workspace", read_roots: ["shared"] },
      { name: "orders", policy_origin: "workspace", declared_by: "workspace", read_roots: ["shared"] },
      { name: "plain", policy_origin: "unset", declared_by: "member", read_roots: [] },
      { name: "broken", policy_origin: null, declared_by: null, read_roots: [] },
    ];
    const d = workspaceReadRootsDisclosure(members);
    expect(d.groups).toEqual([
      { roots: ["shared"], relativeTo: "the workspace root", readBy: ["web", "orders"] },
      { roots: ["../api-docs"], relativeTo: "api", readBy: [] },
    ]);
    expect(d.unreadable).toEqual(["broken"]);
    expect(consentEntries(d)).toEqual([
      "the workspace root: shared",
      "api: ../api-docs",
      "unreadable: broken",
    ]);
  });

  it("never calls an absolute root relative to anything (S-485 review)", () => {
    const d = workspaceReadRootsDisclosure([
      { name: "api", policy_origin: "member", declared_by: "member", read_roots: ["../api-docs", "/abs/docs"] },
      { name: "web", policy_origin: "workspace", declared_by: "workspace", read_roots: ["shared", "C:\\srv\\specs"] },
    ]);
    expect(d.groups).toEqual([
      { roots: ["shared"], relativeTo: "the workspace root", readBy: ["web"] },
      { roots: ["C:\\srv\\specs"], relativeTo: null, readBy: ["web"] },
      { roots: ["../api-docs"], relativeTo: "api", readBy: [] },
      { roots: ["/abs/docs"], relativeTo: null, readBy: ["api"] },
    ]);
    // An absolute entry is the same directory whoever declared it.
    expect(consentEntries(d)).toContain("/abs/docs");
    // The member chat's inherited table splits the same way; its own table keeps
    // one group in declared order.
    const inherited = memberReadRootsDisclosure({
      ...READY,
      policyOrigin: "workspace",
      policy: { ...POLICY, read_roots: ["/srv/specs", "../d"] },
    });
    expect(inherited.groups).toEqual([
      { roots: ["../d"], relativeTo: "the workspace root", readBy: [] },
      { roots: ["/srv/specs"], relativeTo: null, readBy: [] },
    ]);
    const own = memberReadRootsDisclosure({ ...READY, policy: { ...POLICY, read_roots: ["/srv/specs", "../d"] } });
    expect(own.groups).toEqual([{ roots: ["/srv/specs", "../d"], relativeTo: null, readBy: [] }]);
  });

  it("asks again when the same root moves to another declaring root", () => {
    const owned = workspaceReadRootsDisclosure([
      { name: "api", policy_origin: "member", declared_by: "member", read_roots: ["docs"] },
    ]);
    const inherited = workspaceReadRootsDisclosure([
      { name: "api", policy_origin: "workspace", declared_by: "workspace", read_roots: ["docs"] },
    ]);
    expect(consentEntries(owned)).not.toEqual(consentEntries(inherited));
  });
});

/** The workspace tier's chat slice, declared (`member` = at the workspace root) or not. */
function tierModel(policy: ChatOrigin, credential: ChatOrigin): WorkspaceChatConfigReadModel {
  return {
    ...configModel(policy, credential),
    config: { error: null },
    chat_key_error: null,
  };
}

describe("workspaceChatReadiness (S-485)", () => {
  it("is ready only when the workspace root declares both halves", () => {
    expect(workspaceChatReadiness(tierModel("member", "member")).ready).toBe(true);
    for (const [p, c] of [
      ["unset", "unset"],
      ["member", "unset"],
      ["unset", "member"],
    ] as const) {
      expect(workspaceChatReadiness(tierModel(p, c)).ready).toBe(false);
    }
  });

  it("names the absent half, the present one, and the workspace-root files", () => {
    expect(workspaceChatReadiness(tierModel("unset", "unset"))).toEqual({
      ready: false,
      unreadable: false,
      absent: "both",
      present: null,
      workspaceFiles: [WORKSPACE_CONFIG_FILE, WORKSPACE_SECRETS_FILE],
    });
    expect(workspaceChatReadiness(tierModel("member", "unset"))).toMatchObject({
      absent: "key",
      present: "model",
      workspaceFiles: [WORKSPACE_SECRETS_FILE],
    });
    expect(workspaceChatReadiness(tierModel("unset", "member"))).toMatchObject({
      absent: "model",
      present: "key",
      workspaceFiles: [WORKSPACE_CONFIG_FILE],
    });
  });

  it("is the unreadable state — never 'not configured' — when a tier file does not parse", () => {
    const model: WorkspaceChatConfigReadModel = {
      effective_chat: null,
      config: { error: "config.toml line 2: expected `]`" },
      chat_key_error: null,
    };
    expect(workspaceChatReadiness(model)).toEqual({
      ready: false,
      unreadable: true,
      faults: ["config.toml line 2: expected `]`"],
    });
  });

  it("words configure-first as the workspace turn's refusal: the root, the half, no member", () => {
    const state = workspaceChatReadiness(tierModel("member", "unset"));
    if (state.ready || state.unreadable) throw new Error("expected configure-first");
    const copy = workspaceConfigureFirstCopy(state, "shop");
    expect(copy.summary).toBe(
      "The workspace chat is not configured yet for the workspace root of shop — no API key is declared there.",
    );
    expect(copy.present).toBe("Its provider model is declared.");
    expect(copy.memberNote).toBe("A member's own [chat] does not configure the workspace chat.");
    expect(copy.action).toBe("Add an API key");
  });
});

describe("readRoots (sprint-79 HF-1) — the declared extra read roots", () => {
  it("is empty when the server omits the key (the default) or sends an empty list", () => {
    expect(readRoots(POLICY)).toEqual([]);
    expect(readRoots({ ...POLICY, read_roots: [] })).toEqual([]);
  });

  it("names each declared entry verbatim, dropping only blank ones", () => {
    expect(readRoots({ ...POLICY, read_roots: ["../logos-docs", " ", "/srv/specs"] })).toEqual([
      "../logos-docs",
      "/srv/specs",
    ]);
  });
});
