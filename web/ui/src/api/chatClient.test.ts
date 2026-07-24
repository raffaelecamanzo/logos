import { afterEach, describe, expect, it, vi } from "vitest";

// Mock the transport seams so the test asserts the request CONTRACT chatClient
// builds (route, body encoding, headers, intent plumbing) without a live fetch or
// the module-load intent token.
vi.mock("../intent.ts", () => ({
  apiMutate: vi.fn(() => Promise.resolve({ ok: true, status: 200 } as Response)),
}));
vi.mock("./client.ts", () => ({
  apiFetch: vi.fn(() => Promise.resolve({})),
}));

import { apiMutate } from "../intent.ts";
import { apiFetch } from "./client.ts";
import {
  CHAT_CLEAR_ROUTE,
  CHAT_ROUTE,
  clearChatHistory,
  fetchChatConfig,
  fetchThreadMessages,
  fetchThreads,
  streamChatTurn,
} from "./chatClient.ts";

const mockMutate = vi.mocked(apiMutate);
const mockFetch = vi.mocked(apiFetch);

afterEach(() => vi.clearAllMocks());

describe("streamChatTurn", () => {
  it("POSTs the form-encoded question with the SSE Accept header over the intent seam", async () => {
    const ctrl = new AbortController();
    await streamChatTurn("what is risky?", null, ctrl.signal);
    expect(mockMutate).toHaveBeenCalledWith(CHAT_ROUTE, {
      headers: { "Content-Type": "application/x-www-form-urlencoded", Accept: "text/event-stream" },
      body: "q=what%20is%20risky%3F",
      signal: ctrl.signal,
    });
  });

  it("omits the thread field for a fresh conversation (byte-identical single-thread body)", async () => {
    await streamChatTurn("hi", null);
    expect(mockMutate.mock.calls[0][1]?.body).toBe("q=hi");
  });

  it("appends the active thread id when continuing a conversation", async () => {
    await streamChatTurn("more", 7);
    expect(mockMutate.mock.calls[0][1]?.body).toBe("q=more&thread=7");
  });
});

describe("fetchThreads", () => {
  it("GETs the same-origin thread list (S-209 read API, no re-added route)", async () => {
    await fetchThreads();
    expect(mockFetch).toHaveBeenCalledWith("chat/threads");
  });
});

describe("fetchThreadMessages", () => {
  it("GETs one thread's transcript by id", async () => {
    await fetchThreadMessages(42);
    expect(mockFetch).toHaveBeenCalledWith("chat/threads/42");
  });
});

describe("clearChatHistory", () => {
  it("POSTs the clear route over the intent seam", async () => {
    await clearChatHistory();
    expect(mockMutate).toHaveBeenCalledWith(CHAT_CLEAR_ROUTE, {});
  });
});

describe("fetchChatConfig", () => {
  it("GETs the same-origin config read-model", async () => {
    await fetchChatConfig();
    expect(mockFetch).toHaveBeenCalledWith("config");
  });
});
