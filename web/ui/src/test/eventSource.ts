// A fake browser `EventSource` for the tests that follow a job's event stream
// (S-739): jsdom has none. `installEventSources()` stubs the global and returns
// the sources opened so far; a test plays the server by calling `emit` (a named
// event with JSON data) or `fail` (a broken stream) on one, and reads `closed` to
// see whether the page stopped listening — a source left open is one a browser
// would reconnect on its own.
import { vi } from "vitest";

export class FakeEventSource {
  readonly url: string;
  closed = false;
  private readonly listeners = new Map<string, ((event: Event) => void)[]>();

  constructor(url: string | URL) {
    this.url = String(url);
  }

  addEventListener(type: string, listener: (event: Event) => void): void {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }

  close(): void {
    this.closed = true;
  }

  /** The server sends `type` with `data` (serialised unless already a string). */
  emit(type: string, data: unknown): void {
    const message = new MessageEvent(type, { data: typeof data === "string" ? data : JSON.stringify(data) });
    for (const listener of this.listeners.get(type) ?? []) listener(message);
  }

  /** The stream breaks. */
  fail(): void {
    for (const listener of this.listeners.get("error") ?? []) listener(new Event("error"));
  }
}

/** Stub the global `EventSource`; the returned list holds every source opened. */
export function installEventSources(): FakeEventSource[] {
  const opened: FakeEventSource[] = [];
  vi.stubGlobal(
    "EventSource",
    class extends FakeEventSource {
      constructor(url: string | URL) {
        super(url);
        opened.push(this);
      }
    },
  );
  return opened;
}
