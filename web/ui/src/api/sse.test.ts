import { describe, expect, it } from "vitest";

import { parseSseBlock, readSseStream, type SseFrame } from "./sse.ts";

/** A ReadableStream that emits the given chunks (as UTF-8) then closes. */
function streamOf(chunks: string[]): ReadableStream<Uint8Array> {
  const enc = new TextEncoder();
  let i = 0;
  return new ReadableStream({
    pull(controller) {
      if (i < chunks.length) {
        controller.enqueue(enc.encode(chunks[i++]));
      } else {
        controller.close();
      }
    },
  });
}

async function collect(chunks: string[]): Promise<SseFrame[]> {
  const frames: SseFrame[] = [];
  await readSseStream(streamOf(chunks), (f) => frames.push(f));
  return frames;
}

describe("parseSseBlock (S-178, FR-UI-19)", () => {
  it("parses the event name and joins multi-line data", () => {
    expect(parseSseBlock("event: page-written\ndata: {\"a\":1}")).toEqual({
      name: "page-written",
      data: '{"a":1}',
    });
    expect(parseSseBlock("event: x\ndata: line1\ndata: line2")).toEqual({
      name: "x",
      data: "line1\nline2",
    });
  });

  it("defaults the event name to `message` when none is given", () => {
    expect(parseSseBlock("data: hi")).toEqual({ name: "message", data: "hi" });
  });

  it("returns null for a keep-alive comment block (no data line)", () => {
    expect(parseSseBlock(": keep-alive")).toBeNull();
    expect(parseSseBlock("event: started")).toBeNull(); // no data: → not a frame
  });
});

describe("readSseStream (S-178, FR-UI-19)", () => {
  it("flushes a trailing block that has no terminating blank line", async () => {
    // The final frame arrives without a trailing `\n\n` — the tail-flush must still
    // yield it.
    const frames = await collect(["event: completed\ndata: {\"pages_written\":1}"]);
    expect(frames).toEqual([{ name: "completed", data: '{"pages_written":1}' }]);
  });

  it("reassembles a frame split across two chunks mid-line", async () => {
    const frames = await collect(["event: page-", "written\ndata: {\"slug\":\"a\"}\n\n"]);
    expect(frames).toEqual([{ name: "page-written", data: '{"slug":"a"}' }]);
  });

  it("skips keep-alive comment blocks between real frames", async () => {
    const frames = await collect([
      "event: started\ndata: {\"total\":1}\n\n",
      ": keep-alive\n\n",
      "event: completed\ndata: {\"pages_written\":1}\n\n",
    ]);
    expect(frames.map((f) => f.name)).toEqual(["started", "completed"]);
  });

  it("is a clean no-op on a null body", async () => {
    const frames: SseFrame[] = [];
    await readSseStream(null, (f) => frames.push(f));
    expect(frames).toEqual([]);
  });
});
