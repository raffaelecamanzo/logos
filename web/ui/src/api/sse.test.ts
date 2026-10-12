import { afterEach, describe, expect, it, vi } from "vitest";

import { createLivenessWatchdog, parseSseBlock, readSseStream, type SseFrame } from "./sse.ts";

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

describe("readSseStream byte arrival (S-626, FR-UI-50, CRA-04)", () => {
  it("reports every received chunk to onBytes, a frameless keepalive comment included", async () => {
    const frames: SseFrame[] = [];
    let chunks = 0;
    await readSseStream(
      streamOf([": keepalive\n\n", ": keepalive\n\n", "event: plan\ndata: {}\n\n"]),
      (f) => frames.push(f),
      () => (chunks += 1),
    );
    // Two comments parse to no frame at all, yet each is liveness the watchdog sees.
    expect(frames.map((f) => f.name)).toEqual(["plan"]);
    expect(chunks).toBe(3);
  });
});

describe("createLivenessWatchdog (S-626, FR-UI-50)", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("raises after the threshold of silence and clears on the next byte", () => {
    vi.useFakeTimers();
    const changes: boolean[] = [];
    const watchdog = createLivenessWatchdog(45_000, (stalled) => changes.push(stalled));
    vi.advanceTimersByTime(44_999);
    expect(changes).toEqual([]);
    vi.advanceTimersByTime(1);
    expect(changes).toEqual([true]);
    watchdog.touch();
    expect(changes).toEqual([true, false]);
    // Re-armed: a second silence raises it again.
    vi.advanceTimersByTime(45_000);
    expect(changes).toEqual([true, false, true]);
    watchdog.stop();
  });

  it("never raises while bytes keep arriving inside the threshold", () => {
    vi.useFakeTimers();
    const changes: boolean[] = [];
    const watchdog = createLivenessWatchdog(45_000, (stalled) => changes.push(stalled));
    for (let s = 0; s < 120; s += 15) {
      vi.advanceTimersByTime(15_000);
      watchdog.touch();
    }
    expect(changes).toEqual([]);
    watchdog.stop();
  });

  it("is silent for good once stopped", () => {
    vi.useFakeTimers();
    const changes: boolean[] = [];
    const watchdog = createLivenessWatchdog(45_000, (stalled) => changes.push(stalled));
    watchdog.stop();
    watchdog.touch(); // a late byte after the stream ended re-arms nothing
    vi.advanceTimersByTime(120_000);
    expect(changes).toEqual([]);
  });
});
