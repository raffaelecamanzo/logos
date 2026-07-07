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
  streamChatTurn,
} from "./chatClient.ts";

const mockMutate = vi.mocked(apiMutate);
const mockFetch = vi.mocked(apiFetch);

afterEach(() => vi.clearAllMocks());

describe("streamChatTurn", () => {
  it("POSTs the form-encoded question with the SSE Accept header over the intent seam", async () => {
    const ctrl = new AbortController();
    await streamChatTurn("what is risky?", ctrl.signal);
    expect(mockMutate).toHaveBeenCalledWith(CHAT_ROUTE, {
      headers: { "Content-Type": "application/x-www-form-urlencoded", Accept: "text/event-stream" },
      body: "q=what%20is%20risky%3F",
      signal: ctrl.signal,
    });
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
