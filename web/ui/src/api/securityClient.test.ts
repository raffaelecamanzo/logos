// The scan-job client (S-673, FR-SC-15, NFR-SE-06): in workspace mode every
// request carries the selected member, so the scan runs, is followed and is
// cancelled on the member the page shows; the writes carry the intent token; a
// `409` is the consent answer, not an error; any other refusal throws with the
// server's status and detail.
import { afterEach, describe, expect, it, vi } from "vitest";

import { setScopedMember } from "../workspace/scope.ts";
import {
  SecurityRequestError,
  cancelInstall,
  cancelScan,
  fetchInstallJob,
  fetchScanJob,
  startInstall,
  startScan,
  suppressFinding,
  unsuppressFinding,
} from "./securityClient.ts";

interface Call {
  url: string;
  method: string;
  intent: string | null;
  body: string | null;
}

/** Stubs fetch answering every request with `status` and `body`. */
function stub(status: number, body: unknown): Call[] {
  const calls: Call[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((url: string, init: RequestInit = {}) => {
      calls.push({
        url,
        method: init.method ?? "GET",
        intent: new Headers(init.headers).get("x-logos-intent"),
        body: typeof init.body === "string" ? init.body : null,
      });
      return Promise.resolve({
        ok: status >= 200 && status < 300,
        status,
        json: () => Promise.resolve(body),
        text: () => Promise.resolve(typeof body === "string" ? body : JSON.stringify(body)),
      } as Response);
    }),
  );
  return calls;
}

afterEach(() => {
  vi.unstubAllGlobals();
  setScopedMember(null);
});

describe("securityClient", () => {
  it("scopes the read, the start and the cancel to the selected member", async () => {
    setScopedMember("api");
    const calls = stub(200, { job: null, joined: false });
    await fetchScanJob();
    await startScan([]);
    await cancelScan();
    expect(calls.map((c) => [c.method, c.url])).toEqual([
      ["GET", "/api/v1/security/scan?repo=api"],
      ["POST", "/api/v1/security/scan?repo=api"],
      ["POST", "/api/v1/security/scan/cancel?repo=api"],
    ]);
    expect(calls.slice(1).map((c) => c.intent)).toEqual(["test-intent-token", "test-intent-token"]);
  });

  it("sends the accepted scanners, and reads a 409 as the consent still needed", async () => {
    const calls = stub(409, { error: "a scan needs consent first", consent_required: ["semgrep", "bandit"] });
    expect(await startScan(["bandit"])).toEqual({ kind: "consent", scanners: ["semgrep", "bandit"] });
    expect(JSON.parse(calls[0].body!)).toEqual({ consent: ["bandit"] });
  });

  it("throws the server's status and detail when a start or a cancel is refused", async () => {
    stub(500, { error: "invalid .logos/config.toml" });
    const start = await startScan([]).catch((e: unknown) => e);
    expect(start).toBeInstanceOf(SecurityRequestError);
    expect((start as SecurityRequestError).status).toBe(500);
    expect((start as SecurityRequestError).detail).toContain("invalid .logos/config.toml");

    stub(403, "cross-origin write rejected");
    const cancel = await cancelScan().catch((e: unknown) => e);
    expect(cancel).toBeInstanceOf(SecurityRequestError);
    expect([(cancel as SecurityRequestError).status, (cancel as SecurityRequestError).detail]).toEqual([403, "cross-origin write rejected"]);
  });

  it("reads no job as null", async () => {
    stub(200, { job: null });
    expect(await fetchScanJob()).toBeNull();
    expect(await cancelScan()).toBeNull();
  });

  it("scopes the triage writes to the selected member, with the intent token and a JSON body (S-672)", async () => {
    setScopedMember("api");
    const fp = "0a".repeat(32);
    const calls = stub(200, { file: ".logos/security-ignore.toml", entry: { fingerprint: fp, reason: "r", expires: null }, entries: 1 });
    const written = await suppressFinding({ fingerprint: fp }, "r", "2027-01-31");
    await unsuppressFinding({ fingerprint: fp });
    // A path entry's selector rides in the same body, narrowing and all (S-770).
    await suppressFinding({ path: "tests/fixtures", scanner: "gitleaks" }, "planted", null);
    await unsuppressFinding({ path: "tests/fixtures", scanner: "gitleaks" });
    expect(written.file).toBe(".logos/security-ignore.toml");
    expect(calls.map((c) => [c.method, c.url, c.intent, c.body])).toEqual([
      ["POST", "/api/v1/security/suppress?repo=api", "test-intent-token", JSON.stringify({ fingerprint: fp, reason: "r", expires: "2027-01-31" })],
      ["POST", "/api/v1/security/unsuppress?repo=api", "test-intent-token", JSON.stringify({ fingerprint: fp })],
      ["POST", "/api/v1/security/suppress?repo=api", "test-intent-token", JSON.stringify({ path: "tests/fixtures", scanner: "gitleaks", reason: "planted", expires: null })],
      ["POST", "/api/v1/security/unsuppress?repo=api", "test-intent-token", JSON.stringify({ path: "tests/fixtures", scanner: "gitleaks" })],
    ]);
  });

  it("throws a refused triage write with the server's reason: the JSON `error`, else the text verbatim", async () => {
    stub(422, { error: "`reason` is empty: a suppression must say why the risk is accepted" });
    const refused = await suppressFinding({ fingerprint: "0a".repeat(32) }, " ", null).catch((e: unknown) => e);
    expect(refused).toBeInstanceOf(SecurityRequestError);
    expect([(refused as SecurityRequestError).status, (refused as SecurityRequestError).detail]).toEqual([
      422,
      "`reason` is empty: a suppression must say why the risk is accepted",
    ]);

    stub(403, "cross-origin write rejected");
    const forbidden = await unsuppressFinding({ fingerprint: "0a".repeat(32) }).catch((e: unknown) => e);
    expect([(forbidden as SecurityRequestError).status, (forbidden as SecurityRequestError).detail]).toEqual([403, "cross-origin write rejected"]);
  });

  it("scopes the install's read, start and cancel to the selected member, with the intent token (S-747)", async () => {
    setScopedMember("api");
    const calls = stub(200, { job: null, joined: false });
    await fetchInstallJob();
    await startInstall({ scan: true, consent: ["trivy 0.74.0"], scan_consent: ["trivy"] });
    await cancelInstall();
    expect(calls.map((c) => [c.method, c.url, c.intent])).toEqual([
      ["GET", "/api/v1/security/install?repo=api", null],
      ["POST", "/api/v1/security/install?repo=api", "test-intent-token"],
      ["POST", "/api/v1/security/install/cancel?repo=api", "test-intent-token"],
    ]);
    expect(JSON.parse(calls[1].body!)).toEqual({ scan: true, consent: ["trivy 0.74.0"], scan_consent: ["trivy"] });
  });

  it("reads an install's 409 with a plan as the dialog to show", async () => {
    const plan = { lines: [], kept: [], scan_consent: ["trivy"] };
    stub(409, { error: "the install needs your consent first", plan });
    expect(await startInstall({ scan: true })).toEqual({ kind: "consent", plan });
  });

  it("throws a 409 that asks nothing — the other job running — as a refusal", async () => {
    stub(409, { error: "a scan is running on this project; wait for it to finish or cancel it, then install" });
    const install = await startInstall({ scan: true }).catch((e: unknown) => e);
    expect(install).toBeInstanceOf(SecurityRequestError);
    expect((install as SecurityRequestError).detail).toContain("a scan is running");

    stub(409, { error: "scanners are being installed for this project" });
    const scan = await startScan([]).catch((e: unknown) => e);
    expect(scan).toBeInstanceOf(SecurityRequestError);
    expect([(scan as SecurityRequestError).status, (scan as SecurityRequestError).detail]).toEqual([409, "scanners are being installed for this project"]);
  });
});
