import { describe, expect, it } from "vitest";

import { DEFAULT_TABLE_PAGE_SIZE } from "./table.constants.ts";
import { DEFAULT_TABLE_PAGE_SIZE as ViaBarrel } from "./index.ts";

describe("DEFAULT_TABLE_PAGE_SIZE (S-195, FR-UI-11)", () => {
  it("is the single shared page size of 20 (CR-051, dropped from 25)", () => {
    expect(DEFAULT_TABLE_PAGE_SIZE).toBe(20);
  });

  it("is re-exported through the components barrel for one import surface", () => {
    expect(ViaBarrel).toBe(DEFAULT_TABLE_PAGE_SIZE);
  });
});
