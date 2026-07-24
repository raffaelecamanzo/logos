import { describe, expect, it } from "vitest";

import { convertMessage, foldPersistedMessages, type ChatMessage } from "./chatRuntime.tsx";
import { initialTurn, type PersistedChatMessage, type TurnState } from "./chatModel.ts";

/** A folded turn with overrides over a fresh turn. */
function turn(over: Partial<TurnState>): TurnState {
  return { ...initialTurn(), ...over };
}

describe("convertMessage", () => {
  it("maps a user message to a text content part", () => {
    const m: ChatMessage = { kind: "user", id: 1, text: "hello" };
    const out = convertMessage(m);
    expect(out.role).toBe("user");
    expect(out.id).toBe("1");
    expect(out.content).toEqual([{ type: "text", text: "hello" }]);
  });

  it("mirrors the assistant answer into a text part for the Copy affordance", () => {
    const m: ChatMessage = { kind: "assistant", id: 2, parentId: 1, turn: turn({ answer: "the answer" }) };
    const out = convertMessage(m);
    expect(out.role).toBe("assistant");
    expect(out.content).toEqual([{ type: "text", text: "the answer" }]);
    // The full folded turn rides on metadata.custom for the custom render path.
    expect((out.metadata?.custom as { turn: TurnState }).turn.answer).toBe("the answer");
  });

  it("reports a still-streaming turn as running", () => {
    const m: ChatMessage = { kind: "assistant", id: 2, parentId: 1, turn: turn({ answer: "x", streaming: true }) };
    expect(convertMessage(m).status).toEqual({ type: "running" });
  });

  it("reports a finalized turn as complete", () => {
    const m: ChatMessage = { kind: "assistant", id: 2, parentId: 1, turn: turn({ answer: "x", finalized: true }) };
    expect(convertMessage(m).status).toEqual({ type: "complete", reason: "stop" });
  });

  it("reports a budget-halted turn as complete (not running)", () => {
    const m: ChatMessage = { kind: "assistant", id: 2, parentId: 1, turn: turn({ halt: "halted: …" }) };
    expect(convertMessage(m).status).toEqual({ type: "complete", reason: "stop" });
  });

  it("reports an errored turn as incomplete", () => {
    const m: ChatMessage = { kind: "assistant", id: 2, parentId: 1, turn: turn({ error: "boom" }) };
    expect(convertMessage(m).status).toEqual({ type: "incomplete", reason: "error" });
  });
});

/** A persisted transcript row with sensible defaults over the S-209 wire shape. */
function row(over: Partial<PersistedChatMessage>): PersistedChatMessage {
  return { id: 1, role: "user", content: "", created_at: 0, tool_traces: [], ...over };
}

describe("foldPersistedMessages", () => {
  /** A monotonic id allocator mirroring the runtime's `++idRef.current`. */
  function allocator(start = 0): () => number {
    let n = start;
    return () => ++n;
  }

  it("folds a restored user+assistant transcript, answer-only and finalized", () => {
    const out = foldPersistedMessages(
      [row({ role: "user", content: "what is risky?" }), row({ role: "assistant", content: "X is." })],
      allocator(),
    );
    expect(out).toHaveLength(2);
    expect(out[0]).toEqual({ kind: "user", id: 1, text: "what is risky?" });
    expect(out[1].kind).toBe("assistant");
    const asst = out[1] as Extract<ChatMessage, { kind: "assistant" }>;
    // The durable answer is restored; the ephemeral plan/chips side-channel is not.
    expect(asst.turn.answer).toBe("X is.");
    expect(asst.turn.finalized).toBe(true);
    expect(asst.turn.plan).toBeNull();
    expect(asst.turn.chips).toEqual([]);
    // The assistant turn points back at the preceding user message (regenerate).
    expect(asst.parentId).toBe(1);
  });

  it("skips internal system/tool rows (not part of the rendered surface)", () => {
    const out = foldPersistedMessages(
      [
        row({ role: "system", content: "you are…" }),
        row({ role: "user", content: "q" }),
        row({ role: "tool", content: "{...}" }),
        row({ role: "assistant", content: "a" }),
      ],
      allocator(),
    );
    expect(out.map((m) => m.kind)).toEqual(["user", "assistant"]);
  });

  it("allocates local ids via the caller's counter (no collision with server rowids)", () => {
    const out = foldPersistedMessages(
      [row({ id: 900, role: "user", content: "q" }), row({ id: 901, role: "assistant", content: "a" })],
      allocator(10),
    );
    // Local ids, not the DB rowids — so subsequently-sent turns never collide.
    expect(out.map((m) => m.id)).toEqual([11, 12]);
  });
});
