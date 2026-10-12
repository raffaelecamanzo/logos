/*
 * The entry point reads the active extensions ONCE, before the first render
 * (S-642, FR-UI-51): importing `main.tsx` against a served-shell DOM requests
 * `GET /api/v1/extensions` and mounts the app.
 */

import { cleanup, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { clearExtensions, getExtensions } from "./extensions/registry.ts";

vi.mock("./App.tsx", () => ({ App: () => <div data-testid="app" /> }));

afterEach(() => {
  cleanup();
  clearExtensions();
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

describe("main.tsx", () => {
  it("starts the one extensions read and mounts the app", async () => {
    const urls: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (url: string) => {
        urls.push(url);
        return { ok: true, status: 200, json: async () => ({ extensions: [] }) } as Response;
      }),
    );
    document.body.innerHTML = '<div id="root"></div>';

    await import("./main.tsx");

    expect(await screen.findByTestId("app")).toBeInTheDocument();
    expect(urls.filter((u) => u.includes("/api/v1/extensions"))).toEqual(["/api/v1/extensions"]);
    await vi.waitFor(() => expect(getExtensions().status).toBe("ready"));
  });
});
