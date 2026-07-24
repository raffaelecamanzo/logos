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
import * as chatClient from "./chatClient.ts";
import {
  CHAT_ROUTE,
  chatThreadDeleteRoute,
  deleteChatThread,
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

describe("deleteChatThread (S-211, FR-UI-26)", () => {
  it("POSTs the per-thread delete route over the intent seam", async () => {
    await deleteChatThread(42);
    expect(mockMutate).toHaveBeenCalledWith("/api/v1/chat/threads/42/delete", {});
  });

  it("addresses exactly the named conversation (never a global wipe)", () => {
    expect(chatThreadDeleteRoute(7)).toBe("/api/v1/chat/threads/7/delete");
    expect(chatThreadDeleteRoute(8)).toBe("/api/v1/chat/threads/8/delete");
  });

  it("returns the raw response so 204 / 404 / fault stay distinguishable", async () => {
    mockMutate.mockResolvedValueOnce({ ok: false, status: 404 } as Response);
    await expect(deleteChatThread(9)).resolves.toMatchObject({ status: 404 });
  });
});

describe("the retired global clear (S-211, ADR-47)", () => {
  it("exports no clear-all helper or route — per-conversation delete is the only path", () => {
    const surface = chatClient as unknown as Record<string, unknown>;
    expect(surface.clearChatHistory).toBeUndefined();
    expect(surface.CHAT_CLEAR_ROUTE).toBeUndefined();
  });
});

describe("fetchChatConfig", () => {
  it("GETs the same-origin config read-model", async () => {
    await fetchChatConfig();
    expect(mockFetch).toHaveBeenCalledWith("config");
  });
});
