import { describe, expect, it } from "vitest";

import { convertMessage, type ChatMessage } from "./chatRuntime.tsx";
import { initialTurn, type TurnState } from "./chatModel.ts";

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
