/*
 * Declared topics on the workspace service map (S-491, CR-157 E, FR-UI-29,
 * FR-WS-36), asserted on the RENDERED map.
 *
 * `WorkspaceView.test.tsx` stubs the canvas down to the set it is handed; this
 * file mounts the REAL ECharts canvas over the recording 2D context
 * (`test/renderedChart.ts`) and reads back what it drew — every stroke and fill
 * with its colour and dash — so "its own style, never the producer→consumer
 * style" is a fact about pixels-to-be, not about a model object. The legend and
 * the panel are DOM, and are read as DOM.
 *
 * The no-declaration baseline (`fixtures/serviceMap.noDeclarations.golden.json`)
 * was captured on the commit BEFORE this story, with this file's own mount, and
 * is compared, never refreshed: there is no update mode. Changing it means
 * editing the JSON in a reviewed commit.
 */
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import type { MemberTopics, TopicDeclaration, XserviceDeclaredTopics } from "../../api/types.ts";
import { SERVICE_MAP_TEXT } from "../../copy/serviceMap.copy.ts";
import { paintMarks, recordCanvas, type CanvasRecording, type DrawnMark } from "../../test/renderedChart.ts";
import { WorkspaceProvider } from "../../workspace/WorkspaceContext.tsx";
import { NO_DECLARED_TOPICS, stubApi } from "../../workspace/testFixtures.ts";
import { EDGE_COLOR } from "../graph/graphModel.ts";
import golden from "./fixtures/serviceMap.noDeclarations.golden.json?raw";
import { WorkspaceView } from "./WorkspaceView.tsx";

/** `api` publishes `orders` and `payments`; `web` subscribes to `orders`. So the
 *  map draws two topic nodes and three broker hops — the observed picture a
 *  declaration is drawn beside. */
const TOPICS: MemberTopics[] = [
  {
    member: "api",
    topics: [
      { topic: "orders", producers: 1, consumers: 0 },
      { topic: "payments", producers: 1, consumers: 0 },
    ],
  },
  { member: "web", topics: [{ topic: "orders", producers: 0, consumers: 1 }] },
];

function declaration(over: Partial<TopicDeclaration>): TopicDeclaration {
  return {
    member: "web",
    topic: "orders",
    provenance: "config-declared",
    key: "spring.kafka.topics.orders",
    file: "src/main/resources/application.yml",
    profile: null,
    shape: "spring-kafka-topics",
    status: "observed",
    ...over,
  };
}

/** The NET-NEW declaration: `web`'s committed configuration names `payments`,
 *  which its code neither publishes nor subscribes to. */
const NET_NEW = declaration({
  topic: "payments",
  key: "app.payments.topic",
  file: "src/main/resources/application-docker.yml",
  profile: "docker",
  shape: "last-segment",
  status: "net-new",
  site: { site: "site-uncaptured" },
});

/** Two already-observed declarations — `api` on `orders` (it publishes) and
 *  `web` on `orders` (it subscribes) — counted beside the panel, never listed. */
const OBSERVED_API = declaration({ member: "api", key: "spring.kafka.template.defaulttopic", shape: "last-segment-suffix" });
const OBSERVED_WEB = declaration({});
/** A second declaration of the observed `web`–`orders` pair, from another file:
 *  so the observed tally is 2 pairs over 3 declarations, and the two figures
 *  cannot be swapped unnoticed. */
const OBSERVED_WEB_DOCKER = declaration({ file: "src/main/resources/application-docker.yml", profile: "docker" });

const DECLARED: XserviceDeclaredTopics = {
  headline: {
    ...NO_DECLARED_TOPICS.headline,
    declared_topic_pairs: 3,
    declarations_considered: 4,
    already_observed: { pairs: 2, declarations: 3 },
    net_new: { ...NO_DECLARED_TOPICS.headline.net_new, pairs: 1, declarations: 1, site_uncaptured: 1 },
    captured_topics: 2,
    members: { members: 2, read: 2, declaring: 2 },
    summary:
      "3 declared topic pairs: 2 already observed, 1 net-new (0 config-only, 0 site-refused, 1 site-uncaptured) — 4 declarations considered over 2 of 2 members",
  },
  topics: [
    { topic: "orders", declarations: [OBSERVED_API, OBSERVED_WEB, OBSERVED_WEB_DOCKER] },
    { topic: "payments", declarations: [NET_NEW] },
  ],
  declared_only: [],
  refused: [],
};

let canvas: CanvasRecording;

beforeEach(() => {
  canvas = recordCanvas();
});
afterEach(() => {
  cleanup();
  canvas.restore();
});

/** Mount the Workspace tab over `opts` and return it once the map has drawn its
 *  broker hops — and, when `settled` holds, whatever else the test waits for. */
async function mount(declaredTopics?: XserviceDeclaredTopics, settled?: () => void) {
  const calls = stubApi({ topics: TOPICS, ...(declaredTopics ? { declaredTopics } : {}) });
  const view = render(
    <WorkspaceProvider>
      <WorkspaceView />
    </WorkspaceProvider>,
  );
  const el = await waitFor(() => {
    const found = view.container.querySelector('[role="application"]');
    expect(found).not.toBeNull();
    return found!;
  });
  // The option is applied from an effect, and the declared layer arrives with
  // its own read: wait for the frame to carry the observed hops.
  await waitFor(() => expect(strokesIn(paintMarks(canvas, el), EDGE_COLOR.publishes).length).toBeGreaterThan(0));
  if (settled) await waitFor(settled);
  return { el, calls, container: view.container, marks: () => paintMarks(canvas, el) };
}

/** The marks of one kind drawn in `color` — compared case-blind, and total over
 *  a palette entry that does not exist (it then matches nothing). */
const marksIn = (kind: DrawnMark["kind"]) => (marks: DrawnMark[], color: string | undefined) =>
  marks.filter((m) => m.kind === kind && m.color.toLowerCase() === String(color).toLowerCase());
const strokesIn = marksIn("stroke");
const fillsIn = marksIn("fill");

/** The dash a mark list draws a colour in, as one comparable string per stroke. */
const dashesOf = (marks: DrawnMark[]) => [...new Set(marks.map((m) => JSON.stringify(m.dash)))].sort();

/** The DOM, with the ids that differ only by mount order normalised. */
function normalisedHtml(container: Element): string {
  return container.innerHTML
    .replace(/_echarts_instance_="[^"]*"/g, '_echarts_instance_="ec"')
    .replace(/zr_\d+/g, "zr")
    .replace(/[:«]r[0-9a-z]+[:»]/g, "«r»");
}

const declaredTopicsWidget = () => screen.getByRole("heading", { name: "Declared topics" }).closest("section")!;

describe("a declared membership is drawn in its own style (S-491 AC1)", () => {
  it("strokes the net-new membership in its own colour and dash, with no arrowhead", async () => {
    const declaredColor = EDGE_COLOR["declares-topic"];
    const { marks } = await mount(DECLARED, () =>
      expect(strokesIn(paintMarks(canvas, document.querySelector('[role="application"]')!), EDGE_COLOR["declares-topic"]).length).toBeGreaterThan(0),
    );
    const frame = marks();
    const declared = strokesIn(frame, declaredColor);
    // One drawn pair (web → payments): one line, and nothing else in its hue —
    // no arrowhead stroked or filled, since a declaration has no direction.
    expect(declared, "exactly the one declared line is stroked in the declared hue").toHaveLength(1);
    expect(fillsIn(frame, declaredColor), "no arrowhead is filled in the declared hue").toHaveLength(0);
    expect(declared[0].dash.length, "the declared line is broken, never solid").toBeGreaterThan(0);

    // The producer→consumer style is the broker teal, solid (publishes) or
    // dashed (subscribes). The declared line shares neither its hue nor any of
    // its dashes.
    const broker = strokesIn(frame, EDGE_COLOR.publishes);
    expect(declaredColor.toLowerCase()).not.toBe(EDGE_COLOR.publishes.toLowerCase());
    expect(dashesOf(broker)).not.toContain(JSON.stringify(declared[0].dash));
  });

  it("draws no declared membership in the producer→consumer style: the broker strokes are those of the map without it", async () => {
    const without = await mount();
    const baseline = strokesIn(without.marks(), EDGE_COLOR.publishes);
    cleanup();

    const withDeclared = await mount(DECLARED, () => expect(screen.getByRole("heading", { name: "Declared topics" })).toBeTruthy());
    const broker = strokesIn(withDeclared.marks(), EDGE_COLOR.publishes);
    expect(broker, "a declaration adds no broker-teal stroke").toEqual(baseline);
  });

  it("gives the class a legend entry drawn as the canvas draws it, apart from the broker rows", async () => {
    await mount(DECLARED, () => expect(screen.getByText("Declared topics", { selector: "span" })).toBeTruthy());
    const legend = screen.getByText("Legend").closest("details")!;
    const row = within(legend).getByText(SERVICE_MAP_TEXT.declaredTopicLegend).closest("li")!;
    const line = row.querySelector("line")!;
    expect(line.getAttribute("stroke")).toBe(EDGE_COLOR["declares-topic"]);
    expect(line.getAttribute("stroke-dasharray")).toBe("2 3");
    const publishes = within(legend).getByText("publishes").closest("li")!.querySelector("line")!;
    expect(line.getAttribute("stroke")).not.toBe(publishes.getAttribute("stroke"));
    expect(within(legend).getByText(DECLARED.headline.summary)).toBeTruthy();
  });
});

describe("the panel lists net-new declarations and counts the observed ones (S-491 AC2)", () => {
  it("lists each net-new declaration with its key, file and profile", async () => {
    await mount(DECLARED, () => expect(declaredTopicsWidget()).toBeTruthy());
    const widget = declaredTopicsWidget();
    const rows = within(widget).getAllByRole("row").slice(1);
    expect(rows.map((r) => within(r).getAllByRole("cell").map((c) => c.textContent))).toEqual([
      [
        "web",
        "payments",
        "app.payments.topic",
        "src/main/resources/application-docker.yml",
        "docker",
        "Broker sites, none on this topic",
      ],
    ]);
    expect(within(widget).getByText(DECLARED.headline.summary)).toBeTruthy();
  });

  it("counts the already-observed declarations beside the list, never as new rows", async () => {
    await mount(DECLARED, () => expect(declaredTopicsWidget()).toBeTruthy());
    const widget = declaredTopicsWidget();
    expect(within(widget).getByText(SERVICE_MAP_TEXT.declaredTopicsObserved(2, 3))).toBeTruthy();
    const table = within(widget).getByRole("table");
    // Neither observed key is a row.
    expect(within(table).queryByText(OBSERVED_API.key)).toBeNull();
    expect(within(table).queryByText(OBSERVED_WEB.key)).toBeNull();
    expect(within(table).getAllByRole("row")).toHaveLength(2); // header + the one net-new row
  });

  it("words each member's broker-site split, with its refusal count, and shows an unknown split as sent", async () => {
    const split = (member: string, site: TopicDeclaration["site"]) =>
      declaration({ member, topic: "payments", key: `${member}.topic`, status: "net-new", site });
    const splits: XserviceDeclaredTopics = {
      ...DECLARED,
      topics: [
        {
          topic: "payments",
          declarations: [
            split("a-config", { site: "config-only" }),
            split("b-refused", {
              site: "site-refused",
              refusals: [
                { reason: "topic-not-literal", symbol: "x" },
                { reason: "topic-not-literal", symbol: "y" },
              ],
            }),
            split("c-uncaptured", { site: "site-uncaptured" }),
            split("d-unread", { site: "site-unread" }),
            split("e-unknown", { site: "site-later" } as unknown as TopicDeclaration["site"]),
          ],
        },
      ],
    };
    await mount(splits, () => expect(declaredTopicsWidget()).toBeTruthy());
    const code = Object.fromEntries(
      within(declaredTopicsWidget())
        .getAllByRole("row")
        .slice(1)
        .map((r) => within(r).getAllByRole("cell"))
        .map((cells) => [cells[0].textContent, cells[5].textContent]),
    );
    expect(code).toEqual({
      "a-config": "No broker site in the code",
      "b-refused": "Broker sites refused (2)",
      "c-uncaptured": "Broker sites, none on this topic",
      "d-unread": "Broker sites not read — split unknown",
      "e-unknown": "site-later",
    });
  });

  it("names the unprofiled base rather than leaving the profile blank", async () => {
    const base = { ...DECLARED, topics: [{ topic: "payments", declarations: [{ ...NET_NEW, profile: null }] }] };
    await mount(base, () => expect(declaredTopicsWidget()).toBeTruthy());
    const row = within(declaredTopicsWidget()).getAllByRole("row")[1];
    expect(within(row).getAllByRole("cell")[4].textContent).toBe("base (every profile)");
  });

  it("states a relation with only observed declarations as having no net-new one, and draws nothing", async () => {
    const onlyObserved: XserviceDeclaredTopics = {
      ...DECLARED,
      headline: { ...DECLARED.headline, net_new: NO_DECLARED_TOPICS.headline.net_new, declared_topic_pairs: 2 },
      topics: [{ topic: "orders", declarations: [OBSERVED_API, OBSERVED_WEB] }],
    };
    const { marks } = await mount(onlyObserved, () => expect(declaredTopicsWidget()).toBeTruthy());
    expect(within(declaredTopicsWidget()).getByText(SERVICE_MAP_TEXT.noNetNewDeclarations)).toBeTruthy();
    expect(within(declaredTopicsWidget()).queryByRole("table")).toBeNull();
    expect(strokesIn(marks(), EDGE_COLOR["declares-topic"])).toHaveLength(0);
    expect(screen.queryByText(SERVICE_MAP_TEXT.declaredTopicLegend)).toBeNull();
  });

  it("names each member it could not read, with its reason, as unknown rather than absent", async () => {
    const partial: XserviceDeclaredTopics = {
      ...DECLARED,
      headline: {
        ...DECLARED.headline,
        members: {
          members: 3,
          read: 2,
          declaring: 2,
          unread: ["billing"],
          unread_reasons: { billing: "declared-topic facts could not be read" },
        },
      },
    };
    await mount(partial, () => expect(declaredTopicsWidget()).toBeTruthy());
    const line = within(declaredTopicsWidget()).getByText(/could not be read for/);
    expect(line.textContent).toBe(
      "Declared-topic facts could not be read for billing (declared-topic facts could not be read) — their declared topics are unknown, not absent.",
    );
  });

  it("keeps the panel when nothing was considered but a member was unread, claiming nothing about net-new", async () => {
    const unreadOnly: XserviceDeclaredTopics = {
      ...NO_DECLARED_TOPICS,
      headline: {
        ...NO_DECLARED_TOPICS.headline,
        members: { members: 2, read: 1, declaring: 0, unread: ["web"] },
      },
    };
    await mount(unreadOnly, () => expect(declaredTopicsWidget()).toBeTruthy());
    const widget = declaredTopicsWidget();
    expect(within(widget).getByText(/could not be read for/).textContent).toContain("web");
    expect(within(widget).queryByText(SERVICE_MAP_TEXT.noNetNewDeclarations)).toBeNull();
  });

  it("states a failed read as unknown, never as a workspace that declares nothing", async () => {
    stubApi({ topics: TOPICS, declaredTopicsStatus: 500 });
    render(
      <WorkspaceProvider>
        <WorkspaceView />
      </WorkspaceProvider>,
    );
    expect(await screen.findByText(/The declared-topic relation could not be read/)).toBeTruthy();
    expect(screen.getByText(/declared\s+topics are unknown, not absent/)).toBeTruthy();
  });
});

describe("a workspace with no declarations renders the map as before (S-491 AC3)", () => {
  it("renders the DOM and the canvas strokes byte-for-byte as the pre-story baseline", async () => {
    const { container, marks, calls } = await mount(undefined, () =>
      expect(document.querySelector('[role="application"]')).not.toBeNull(),
    );
    // The relation was read — the map has topics — and declared nothing.
    await waitFor(() => expect(calls().some((u) => u.includes("workspace/declared-topics"))).toBe(true));
    // Let the empty answer land before comparing.
    await new Promise((r) => setTimeout(r, 0));
    const now = { html: normalisedHtml(container), marks: marks() };
    expect(now).toEqual(JSON.parse(golden));
  });

  it("never reads the relation over a workspace with no topic", async () => {
    const calls = stubApi({});
    render(
      <WorkspaceProvider>
        <WorkspaceView />
      </WorkspaceProvider>,
    );
    await screen.findByRole("application");
    expect(calls().some((u) => u.includes("workspace/declared-topics"))).toBe(false);
  });
});
