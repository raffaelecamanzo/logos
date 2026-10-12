/*
 * Shared Server-Sent Events reader (S-178, FR-UI-19) — the transport-agnostic half
 * of every SSE consumer on the SPA.
 *
 * The chat surface (S-190) consumes SSE over an intent-guarded `POST` via a streamed
 * `fetch` body; the wiki-generation surface (S-178) does the same. Both need the
 * identical block-parsing + stream-reading loop, so it lives here as a small,
 * DOM-free module: it operates on a raw `ReadableStream`, so it is driven identically
 * by the browser and by a test's hand-built stream. It holds NO network and NO
 * view state — each consumer folds the parsed frames into its own reducer. The
 * parser is pure; the liveness watchdog below is the one stateful part (a timer).
 *
 * Liveness (S-626, FR-UI-50, CR-204 CRA-04): a server keepalive is an SSE comment,
 * which parses to no frame, so a consumer that only sees `onFrame` cannot tell a
 * live-but-quiet stream from a dead one. The reader is the one layer that sees raw
 * byte arrival, so it reports it through `onBytes`, and
 * {@link createLivenessWatchdog} turns that into a silence signal.
 */

/** One parsed SSE frame: the `event:` name and the joined `data:` payload. */
export interface SseFrame {
  name: string;
  data: string;
}

/**
 * Parse one SSE block (a run of lines up to a blank line) into a {@link SseFrame},
 * or `null` when the block carries no `data:` line (a bare keep-alive comment). The
 * default event name is `"message"` (the SSE spec default), matching the wire.
 */
export function parseSseBlock(block: string): SseFrame | null {
  let name = "message";
  const dataParts: string[] = [];
  for (const line of block.split("\n")) {
    if (line === "" || line.charAt(0) === ":") continue; // blank or keep-alive comment
    if (line.startsWith("event:")) {
      name = line.slice(6).trim();
    } else if (line.startsWith("data:")) {
      dataParts.push(line.slice(5).replace(/^ /, ""));
    }
  }
  return dataParts.length > 0 ? { name, data: dataParts.join("\n") } : null;
}

/**
 * Read an SSE response body to completion, invoking `onFrame` for each parsed frame.
 * Decodes incrementally, splits on the `\n\n` block separator, and flushes the tail.
 * Operates on the raw `ReadableStream` so it is driven identically by the browser and
 * by a test's hand-built stream. A `null` body (no stream) is a clean no-op.
 *
 * `onBytes`, when given, is called for every non-empty chunk read — before parsing,
 * so a keepalive comment that yields no frame still counts as liveness.
 */
export async function readSseStream(
  body: ReadableStream<Uint8Array> | null,
  onFrame: (frame: SseFrame) => void,
  onBytes?: () => void,
): Promise<void> {
  if (!body) return;
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  for (;;) {
    const chunk = await reader.read();
    if (chunk.done) break;
    if (chunk.value.byteLength > 0) onBytes?.();
    buffer += decoder.decode(chunk.value, { stream: true });
    let sep: number;
    while ((sep = buffer.indexOf("\n\n")) !== -1) {
      const block = buffer.slice(0, sep);
      buffer = buffer.slice(sep + 2);
      const frame = parseSseBlock(block);
      if (frame) onFrame(frame);
    }
  }
  buffer += decoder.decode(); // flush any partial multi-byte char the streaming decoder held
  if (buffer.trim()) {
    const frame = parseSseBlock(buffer);
    if (frame) onFrame(frame);
  }
}

/** A silence detector over a byte stream (S-626, FR-UI-50). */
export interface LivenessWatchdog {
  /** Bytes arrived: clear a raised stall and re-arm the threshold. */
  touch(): void;
  /** Disarm for good — the stream ended or was abandoned. Later touches are no-ops. */
  stop(): void;
}

/**
 * Arm a watchdog that calls `onStallChange(true)` once `thresholdMs` pass with no
 * {@link LivenessWatchdog.touch}, and `onStallChange(false)` when a later touch
 * ends that silence. It reports transitions only, so a busy stream costs one timer
 * re-arm per chunk and no consumer update. Armed on creation, so the wait for the
 * response head counts as silence too.
 */
export function createLivenessWatchdog(
  thresholdMs: number,
  onStallChange: (stalled: boolean) => void,
): LivenessWatchdog {
  let stalled = false;
  let stopped = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const arm = () => {
    timer = setTimeout(() => {
      stalled = true;
      onStallChange(true);
    }, thresholdMs);
  };
  arm();
  return {
    touch() {
      if (stopped) return;
      clearTimeout(timer);
      if (stalled) {
        stalled = false;
        onStallChange(false);
      }
      arm();
    },
    stop() {
      stopped = true;
      clearTimeout(timer);
    },
  };
}
