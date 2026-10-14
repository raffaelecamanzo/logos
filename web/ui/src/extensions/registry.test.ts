/*
 * The extension UI registry (S-642, FR-UI-51): an entry is mounted only when the
 * server reports its extension active, lists the entry in its manifest, AND the bundle
 * carries the code for it.
 */

import { afterEach, describe, expect, it } from "vitest";

import { extensionsInfo, STUB_ID, STUB_NAV, STUB_UI, StubAppView, StubView, stubInfo } from "../test/extensions.tsx";
import {
  clearExtensions,
  getExtensions,
  mountedConfigFields,
  mountedConfigPanels,
  mountedIcon,
  mountedNavEntries,
  mountedViews,
  ownsRegisteredPath,
  registerExtensionUi,
  setActiveExtensions,
} from "./registry.ts";
import { STUB_FIELDS, STUB_FIELDS_UI, STUB_MEMBER_GROUPS, STUB_WORKSPACE_GROUPS } from "./fixtures/stubConfigFields.ts";

afterEach(clearExtensions);

describe("nothing is mounted until the server says so", () => {
  it("is empty and `loading` before the boot read, even with code registered", () => {
    registerExtensionUi(STUB_UI);
    expect(getExtensions().status).toBe("loading");
    expect(mountedNavEntries()).toEqual([]);
    expect(mountedViews()).toEqual([]);
    expect(mountedIcon("stub")).toBeUndefined();
    expect(mountedConfigPanels("member")).toEqual([]);
  });

  it("mounts the nav entry, view, icon and config panel of an active, registered extension", () => {
    registerExtensionUi(STUB_UI);
    setActiveExtensions(extensionsInfo(stubInfo()));
    expect(getExtensions().status).toBe("ready");
    expect(mountedNavEntries()).toEqual([STUB_NAV]);
    expect(mountedViews()).toEqual([["/stub", StubView]]);
    expect(mountedIcon("stub")).toBe(STUB_UI.icons?.stub);
    expect(mountedConfigPanels("member").map((p) => p.panel.id)).toEqual(["stub-config"]);
    expect(mountedConfigPanels("workspace").map((p) => p.panel.id)).toEqual(["stub-config"]);
  });

  it("mounts nothing for a registered extension the server does not list", () => {
    registerExtensionUi(STUB_UI);
    setActiveExtensions(extensionsInfo());
    expect(mountedNavEntries()).toEqual([]);
    expect(mountedViews()).toEqual([]);
    expect(mountedConfigPanels("member")).toEqual([]);
    // The code is still known, which is how its routes are told from unknown ones.
    expect(ownsRegisteredPath("/stub")).toBe(true);
  });

  it("mounts nothing for an active extension that reports no UI manifest", () => {
    registerExtensionUi(STUB_UI);
    setActiveExtensions(extensionsInfo(stubInfo({ ui: null })));
    expect(mountedNavEntries()).toEqual([]);
    expect(mountedViews()).toEqual([]);
  });

  it("mounts nothing for an active extension the bundle carries no code for", () => {
    setActiveExtensions(extensionsInfo(stubInfo()));
    expect(mountedNavEntries()).toEqual([]);
    expect(mountedConfigPanels("member")).toEqual([]);
  });

  it("treats a failed boot read as nothing active — and settles", () => {
    registerExtensionUi(STUB_UI);
    setActiveExtensions(null);
    expect(getExtensions().status).toBe("ready");
    expect(mountedNavEntries()).toEqual([]);
  });
});

describe("each entry is mounted on its own manifest line", () => {
  it("mounts a view only when the manifest lists its route (a route nobody lists stays unmounted)", () => {
    registerExtensionUi(STUB_UI);
    setActiveExtensions(extensionsInfo(stubInfo()));
    // `/stub-app` is registered code but neither a manifest view nor a nav path.
    expect(mountedViews().map(([route]) => route)).toEqual(["/stub"]);
    expect(ownsRegisteredPath("/stub-app")).toBe(true);
  });

  it("mounts a view the manifest lists without a sidebar entry (a route a view owns)", () => {
    registerExtensionUi(STUB_UI);
    const info = stubInfo();
    setActiveExtensions(
      extensionsInfo({ ...info, ui: { ...info.ui!, nav: [], views: ["/stub-app"] } }),
    );
    expect(mountedNavEntries()).toEqual([]);
    expect(mountedViews()).toEqual([["/stub-app", StubAppView]]);
  });

  it("skips a nav entry whose route has no registered view — never a link to nothing", () => {
    registerExtensionUi(STUB_UI);
    const info = stubInfo();
    setActiveExtensions(
      extensionsInfo({
        ...info,
        ui: { ...info.ui!, nav: [STUB_NAV, { ...STUB_NAV, id: "ghost", path: "/ghost" }] },
      }),
    );
    expect(mountedNavEntries().map((n) => n.id)).toEqual(["stub"]);
  });

  it("carries replaced_in_workspace_by across as the server sent it", () => {
    registerExtensionUi(STUB_UI);
    const info = stubInfo();
    setActiveExtensions(
      extensionsInfo({
        ...info,
        ui: { ...info.ui!, nav: [{ ...STUB_NAV, replaced_in_workspace_by: "stub-app" }] },
      }),
    );
    expect(mountedNavEntries()[0].replaced_in_workspace_by).toBe("stub-app");
  });

  it("mounts a config panel only in the Config views its component is registered for", () => {
    registerExtensionUi({
      ...STUB_UI,
      configPanels: { "stub-config": { member: STUB_UI.configPanels!["stub-config"].member } },
    });
    setActiveExtensions(extensionsInfo(stubInfo()));
    expect(mountedConfigPanels("member")).toHaveLength(1);
    expect(mountedConfigPanels("workspace")).toEqual([]);
  });

  it("mounts a config panel only when the manifest lists it", () => {
    registerExtensionUi(STUB_UI);
    const info = stubInfo();
    setActiveExtensions(extensionsInfo({ ...info, ui: { ...info.ui!, config_panels: [] } }));
    expect(mountedConfigPanels("member")).toEqual([]);
  });

  it("gives an icon only to the nav ids the manifest mounts", () => {
    registerExtensionUi({ ...STUB_UI, icons: { stub: STUB_UI.icons!.stub, other: STUB_UI.icons!.stub } });
    setActiveExtensions(extensionsInfo(stubInfo()));
    expect(mountedIcon("stub")).toBeDefined();
    expect(mountedIcon("other")).toBeUndefined();
  });

  it("gives an id the icon of the extension whose entry won it, across extensions", () => {
    const entry = { ...STUB_NAV, id: "x", path: "/x" };
    const iconA = () => null;
    const iconB = () => null;
    registerExtensionUi({ id: "a", icons: { x: iconA }, views: { "/x": StubView } });
    registerExtensionUi({ id: "b", icons: { x: iconB }, views: { "/x": StubView } });
    const ui = { nav: [entry], views: [], config_panels: [] };
    setActiveExtensions(extensionsInfo(stubInfo({ id: "a", ui }), stubInfo({ id: "b", ui })));
    expect(mountedIcon("x")).toBe(iconA);
    expect(mountedNavEntries().map((n) => n.id)).toEqual(["x", "x"]);
  });

  it("mounts an extension once however often the server lists it", () => {
    registerExtensionUi(STUB_UI);
    setActiveExtensions(extensionsInfo(stubInfo(), stubInfo()));
    expect(mountedNavEntries()).toHaveLength(1);
  });
});

describe("registration", () => {
  it("refuses a second registration under one id", () => {
    registerExtensionUi(STUB_UI);
    expect(() => registerExtensionUi(STUB_UI)).toThrow(`"${STUB_ID}" is registered twice`);
  });

  it("matches a registered route as a whole path segment, not a prefix of the name", () => {
    registerExtensionUi(STUB_UI);
    expect(ownsRegisteredPath("/stub")).toBe(true);
    expect(ownsRegisteredPath("/stub/page/x")).toBe(true);
    // One character past the name: a prefix test would own it.
    expect(ownsRegisteredPath("/stubby")).toBe(false);
    expect(ownsRegisteredPath("/stu")).toBe(false);
    expect(ownsRegisteredPath("/")).toBe(false);
  });
});

describe("config field groups (S-730, FR-UI-54)", () => {
  it("mounts nothing before the boot read, and nothing for a build with no extension", () => {
    registerExtensionUi(STUB_FIELDS_UI);
    expect(mountedConfigFields("member")).toEqual([]);
    clearExtensions();
    setActiveExtensions(extensionsInfo());
    expect(mountedConfigFields("member")).toEqual([]);
    expect(mountedConfigFields("workspace")).toEqual([]);
  });

  it("mounts an active extension's groups per scope, keyed to the panel the manifest lists", () => {
    registerExtensionUi(STUB_FIELDS_UI);
    setActiveExtensions(extensionsInfo(stubInfo()));
    const member = mountedConfigFields("member");
    expect(member.map((g) => g.group)).toEqual(STUB_MEMBER_GROUPS);
    expect(member.every((g) => g.scope === "member" && g.panel.id === "stub-config")).toBe(true);
    expect(mountedConfigFields("workspace").map((g) => g.group)).toEqual(STUB_WORKSPACE_GROUPS);
  });

  it("mounts no group for an inactive extension, nor for a panel the manifest does not list", () => {
    registerExtensionUi(STUB_FIELDS_UI);
    setActiveExtensions(extensionsInfo());
    expect(mountedConfigFields("member")).toEqual([]);
    const info = stubInfo();
    setActiveExtensions(extensionsInfo({ ...info, ui: { ...info.ui!, config_panels: [] } }));
    expect(mountedConfigFields("member")).toEqual([]);
  });

  it("mounts a group only in the scopes it is registered for", () => {
    registerExtensionUi({ ...STUB_UI, configFields: { "stub-config": { member: STUB_MEMBER_GROUPS } } });
    setActiveExtensions(extensionsInfo(stubInfo()));
    expect(mountedConfigFields("member")).toHaveLength(1);
    expect(mountedConfigFields("workspace")).toEqual([]);
  });

  it("offers only the fields in the panel's table or a sub-table of it, dropping a group left empty", () => {
    const at = (table: string) => ({ ...STUB_FIELDS[0], table, key: `k_${table}` });
    registerExtensionUi({
      ...STUB_UI,
      configFields: {
        "stub-config": {
          member: [
            { legend: "kept", fields: [at("stub"), at("stub.sub"), at("stubby"), at(""), at("other")] },
            { legend: "dropped", fields: [at("other")] },
          ],
        },
      },
    });
    setActiveExtensions(extensionsInfo(stubInfo()));
    const groups = mountedConfigFields("member");
    expect(groups.map((g) => g.group.legend)).toEqual(["kept"]);
    // `stubby` is one character past the panel's table: a prefix test would keep it.
    expect(groups[0].group.fields.map((f) => f.table)).toEqual(["stub", "stub.sub"]);
  });
});

describe("config field groups never reach the core's top-level keys (S-730 review)", () => {
  it("offers no field for a panel whose table is empty", () => {
    registerExtensionUi({ ...STUB_UI, configFields: { "stub-config": { member: [{ fields: [{ ...STUB_FIELDS[0], table: "" }] }] } } });
    const info = stubInfo();
    setActiveExtensions(extensionsInfo({ ...info, ui: { ...info.ui!, config_panels: info.ui!.config_panels.map((p) => ({ ...p, table: "" })) } }));
    expect(mountedConfigFields("member")).toEqual([]);
  });
});
