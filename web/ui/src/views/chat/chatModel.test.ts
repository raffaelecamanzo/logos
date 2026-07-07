import { afterEach, describe, expect, it } from "vitest";

import {
  ANTHROPIC_HOST,
  applyFrame,
  boundNote,
  CONSENT_KEY,
  endpointHost,
  hasConsent,
  hostOf,
  initialTurn,
  isConfigured,
  parseSseBlock,
  readSseStream,
  rememberConsent,
  roleLabel,
  turnEndedEmpty,
  type ChatConfigReadModel,
  type ChatPolicy,
  type SseFrame,
  type TurnState,
} from "./chatModel.ts";

const POLICY: ChatPolicy = {
  provider: "openai",
  model: "openrouter/some-model",
  base_url: "https://openrouter.ai/api/v1",
  max_tool_calls: 24,
  max_subagent_tool_calls: 8,
  max_replans: 3,
};

function configModel(overrides: Partial<ChatPolicy>, keyPresent: boolean): ChatConfigReadModel {
  return {
    config: { parsed: { chat: { ...POLICY, ...overrides } } },
    chat_key: { present: keyPresent, last4: keyPresent ? "1234" : null },
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

describe("isConfigured", () => {
  it("needs both a model and a present key", () => {
    expect(isConfigured(configModel({}, true))).toBe(true);
    expect(isConfigured(configModel({}, false))).toBe(false);
    expect(isConfigured(configModel({ model: null }, true))).toBe(false);
    expect(isConfigured(configModel({ model: "  " }, true))).toBe(false);
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

  it("remembers an acknowledgement across reads", () => {
    expect(hasConsent()).toBe(false);
    rememberConsent();
    expect(window.localStorage.getItem(CONSENT_KEY)).toBe("1");
    expect(hasConsent()).toBe(true);
  });
});
