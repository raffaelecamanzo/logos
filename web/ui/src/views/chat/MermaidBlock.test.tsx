/*
 * The chat Mermaid viewer (S-302, FR-UI-32, FR-WK-15).
 *
 * The render seam is MOCKED exactly as the wiki tests mock it, so jsdom never loads
 * the 3 MB vendored UMD bundle; each test decides what the seam "did" to the target
 * by writing into it, which is precisely the contract the component depends on
 * (`renderMermaidIn` resolves the same way whether the bundle loaded, whether the
 * diagram parsed, or whether nothing happened at all).
 */

import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../wiki/mermaid.ts", () => ({
  renderMermaidIn: vi.fn(() => Promise.resolve()),
  VENDORED_MERMAID_URL: "/assets/vendor/mermaid.min.js",
}));

import { ThemeProvider } from "../../theme/ThemeProvider.tsx";
import { useTheme } from "../../theme/theme.ts";
import { renderMermaidIn } from "../wiki/mermaid.ts";
import { MarkdownAnswer } from "./MarkdownAnswer.tsx";

const mockRender = vi.mocked(renderMermaidIn);

const DIAGRAM = "graph LR;\n  a-->b";
const FENCE = `Here is the shape:\n\n\`\`\`mermaid\n${DIAGRAM}\n\`\`\`\n`;

/** The seam's target: the global `.mermaid` element the shared seam scans for. */
function target(): HTMLElement {
  const el = document.querySelector<HTMLElement>(".mermaid");
  if (!el) throw new Error("no .mermaid render target in the document");
  return el;
}

/** Stand in for a SUCCESSFUL bundle render, faithfully: paint an SVG into EVERY
 *  unprocessed `.mermaid` in the container and set Mermaid's own `data-processed`
 *  latch — and, like the real `mermaid.run`, SKIP a node that already carries it.
 *  Honouring the latch is what makes the component's `removeAttribute` load-bearing
 *  in tests: without it, deleting that line would leave every test green. */
function succeed(svg = "<svg><g class='node'></g></svg>") {
  mockRender.mockImplementation((container: HTMLElement) => {
    for (const el of container.querySelectorAll<HTMLElement>(".mermaid")) {
      if (el.getAttribute("data-processed") === "true") continue;
      el.setAttribute("data-processed", "true");
      el.innerHTML = svg;
    }
    return Promise.resolve();
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  // Default: the seam resolves having done nothing — the bundle-unavailable case.
  mockRender.mockImplementation(() => Promise.resolve());
});
afterEach(() => {
  cleanup();
  // Restores the clipboard spies individual tests install. Safe because `beforeEach`
  // re-installs the seam's default implementation afterwards.
  vi.restoreAllMocks();
});

describe("MermaidBlock — the diagram branch (FR-UI-32)", () => {
  it("routes a ```mermaid fence through the shared vendored seam", async () => {
    render(<MarkdownAnswer text={FENCE} />);
    await waitFor(() => expect(mockRender).toHaveBeenCalledTimes(1));
    // The seam is handed a CONTAINER; it scans for `.mermaid` itself, so the target
    // must be a descendant — the same contract WikiPageBody relies on.
    const container = mockRender.mock.calls[0][0];
    expect(container.querySelector(".mermaid")).toBe(target());
  });

  it("seeds the target with the escaped diagram source before the bundle answers", () => {
    // The mock never resolves, so the component is frozen at the pre-render state.
    mockRender.mockImplementation(() => new Promise<void>(() => {}));
    render(<MarkdownAnswer text={FENCE} />);
    expect(target().textContent).toBe(DIAGRAM);
    // Escaped by construction: the source is written via textContent, so no markup
    // from the answer can become live DOM.
    expect(target().querySelector("svg")).toBeNull();
  });

  it("renders the diagram SVG once the bundle succeeds", async () => {
    succeed();
    render(<MarkdownAnswer text={FENCE} />);
    await waitFor(() => expect(target().querySelector("svg")).not.toBeNull());
    expect(target().getAttribute("data-processed")).toBe("true");
    // The prose around the fence still renders as markdown.
    expect(screen.getByText("Here is the shape:")).toBeInTheDocument();
  });

  it("leaves the escaped source visible when the bundle is unavailable — never a blank", async () => {
    // The default mock: resolves without touching the target (script load failed).
    render(<MarkdownAnswer text={FENCE} />);
    await waitFor(() => expect(screen.getByText(/could not be rendered/)).toBeInTheDocument());
    expect(target().textContent).toBe(DIAGRAM);
    expect(target().hasAttribute("data-processed")).toBe(false);
  });

  it("restores the escaped source when the diagram is unparseable", async () => {
    // Mermaid does not blank an unparseable diagram — it paints its own error
    // graphic. That is worse than the source, so it must be replaced.
    succeed('<svg aria-roledescription="error"><g class="error-icon"></g></svg>');
    render(<MarkdownAnswer text={FENCE} />);
    await waitFor(() => expect(screen.getByText(/could not be rendered/)).toBeInTheDocument());
    expect(target().textContent).toBe(DIAGRAM);
    expect(target().querySelector("svg")).toBeNull();
  });

  it("treats Mermaid's error-text graphic as a failure too", async () => {
    succeed('<svg><text class="error-text">Syntax error</text></svg>');
    render(<MarkdownAnswer text={FENCE} />);
    await waitFor(() => expect(screen.getByText(/could not be rendered/)).toBeInTheDocument());
    expect(target().textContent).toBe(DIAGRAM);
    // Mermaid's latch must be actively CLEARED on the failure path, or the restored
    // source keeps the rendered-state styling and can never be re-rendered.
    expect(target().hasAttribute("data-processed")).toBe(false);
  });

  it("KEEPS a rendered diagram whose own classes merely contain 'error'", async () => {
    // Mermaid stamps author-chosen names into the SVG: a `classDef errorNode` lands
    // on a node's class list, and every edge carries `LS-<from> LE-<to>` built from
    // the raw node ids — so `A --> error_handler` puts "error" in a class attribute
    // of a perfectly good diagram. Only Mermaid's OWN error graphic counts.
    succeed(
      '<svg><g class="node default errorNode"><text>recover</text></g>' +
        '<path class="flowchart-link LS-A LE-error_handler"></path></svg>',
    );
    render(<MarkdownAnswer text={FENCE} />);
    await waitFor(() => expect(target().querySelector("svg")).not.toBeNull());
    expect(screen.queryByText(/could not be rendered/)).not.toBeInTheDocument();
    expect(target().getAttribute("data-processed")).toBe("true");
  });

  it("does not claim a render failure for an EMPTY mermaid fence", async () => {
    render(<MarkdownAnswer text={"```mermaid\n```\n"} />);
    // Nothing to draw and nothing to fall back on, so no render is attempted and no
    // failure is asserted — the one shape where "source stays visible" would
    // otherwise degenerate into a blank under a false failure note.
    await waitFor(() => expect(screen.getByText("mermaid")).toBeInTheDocument());
    expect(mockRender).not.toHaveBeenCalled();
    expect(screen.queryByText(/could not be rendered/)).not.toBeInTheDocument();
  });

  it("keeps the fallback source ESCAPED — markup in a fence never becomes DOM", async () => {
    const hostile = 'graph LR;\n  a["<img src=x onerror=alert(1)>"]';
    render(<MarkdownAnswer text={"```mermaid\n" + hostile + "\n```\n"} />);
    await waitFor(() => expect(screen.getByText(/could not be rendered/)).toBeInTheDocument());
    expect(document.querySelectorAll("img")).toHaveLength(0);
    expect(target().textContent).toBe(hostile);
  });

  it("renders every diagram in a multi-diagram answer, each with its own zoom", async () => {
    const user = userEvent.setup();
    succeed();
    render(
      <MarkdownAnswer
        text={"```mermaid\ngraph LR;\n  a-->b\n```\n\n```mermaid\ngraph TD;\n  x-->y\n```\n"}
      />,
    );
    await waitFor(() => expect(document.querySelectorAll(".mermaid svg")).toHaveLength(2));

    // Each block owns its own host and its own zoom state.
    const zoomIns = screen.getAllByRole("button", { name: "Zoom in" });
    expect(zoomIns).toHaveLength(2);
    await user.click(zoomIns[0]);
    expect(screen.getByText("125%")).toBeInTheDocument();
    expect(screen.getByText("100%")).toBeInTheDocument();
  });
});

describe("MermaidBlock — zoom, toggle, and copy (FR-UI-32)", () => {
  it("zooms in and out along a bounded ladder and resets to 100%", async () => {
    const user = userEvent.setup();
    succeed();
    render(<MarkdownAnswer text={FENCE} />);
    await waitFor(() => expect(mockRender).toHaveBeenCalled());

    // The readout AND the scale wrapper's class are both asserted: the label alone
    // would stay green if every step mapped to the same class and zoom shipped dead.
    // (A deliberate exception to this suite's "no class-name assertions" rule — the
    // ladder's whole mechanism IS the class, since the CSP forbids an inline style.)
    const scale = () => target().parentElement!.className;
    expect(screen.getByText("100%")).toBeInTheDocument();
    expect(scale()).toContain("zoom100");
    // Reset is spent at the default step.
    expect(screen.getByRole("button", { name: "Reset zoom" })).toBeDisabled();

    await user.click(screen.getByRole("button", { name: "Zoom in" }));
    expect(screen.getByText("125%")).toBeInTheDocument();
    expect(scale()).toContain("zoom125");
    await user.click(screen.getByRole("button", { name: "Zoom out" }));
    await user.click(screen.getByRole("button", { name: "Zoom out" }));
    expect(screen.getByText("80%")).toBeInTheDocument();
    expect(scale()).toContain("zoom80");

    await user.click(screen.getByRole("button", { name: "Reset zoom" }));
    expect(screen.getByText("100%")).toBeInTheDocument();
    expect(scale()).toContain("zoom100");
    expect(screen.getByRole("button", { name: "Reset zoom" })).toBeDisabled();
  });

  it("stops at both ends of the zoom ladder rather than running off it", async () => {
    const user = userEvent.setup();
    succeed();
    render(<MarkdownAnswer text={FENCE} />);
    await waitFor(() => expect(mockRender).toHaveBeenCalled());

    const zoomOut = screen.getByRole("button", { name: "Zoom out" });
    for (let i = 0; i < 6; i += 1) if (!(zoomOut as HTMLButtonElement).disabled) await user.click(zoomOut);
    expect(screen.getByText("50%")).toBeInTheDocument();
    expect(zoomOut).toBeDisabled();

    const zoomIn = screen.getByRole("button", { name: "Zoom in" });
    for (let i = 0; i < 12; i += 1) if (!(zoomIn as HTMLButtonElement).disabled) await user.click(zoomIn);
    expect(screen.getByText("300%")).toBeInTheDocument();
    expect(zoomIn).toBeDisabled();
  });

  it("toggles between the diagram and the raw source, hiding zoom in source mode", async () => {
    const user = userEvent.setup();
    succeed();
    render(<MarkdownAnswer text={FENCE} />);
    await waitFor(() => expect(target().querySelector("svg")).not.toBeNull());

    await user.click(screen.getByRole("button", { name: "Show source" }));
    // The diagram target is gone; the verbatim source is on screen as text.
    expect(document.querySelector(".mermaid")).toBeNull();
    expect(screen.getByText(/graph LR;/)).toBeInTheDocument();
    // Zoom is meaningless over source text, so it is not offered.
    expect(screen.queryByRole("button", { name: "Zoom in" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Reset zoom" })).not.toBeInTheDocument();

    // Toggling back re-renders the diagram through the seam.
    await user.click(screen.getByRole("button", { name: "Show diagram" }));
    await waitFor(() => expect(target().querySelector("svg")).not.toBeNull());
    expect(screen.getByRole("button", { name: "Zoom in" })).toBeInTheDocument();
  });

  it("copies the RAW fence source in diagram mode", async () => {
    const user = userEvent.setup();
    const writeText = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue();
    succeed();
    render(<MarkdownAnswer text={FENCE} />);
    await waitFor(() => expect(target().querySelector("svg")).not.toBeNull());

    await user.click(screen.getByRole("button", { name: "Copy code" }));
    expect(writeText).toHaveBeenCalledWith(DIAGRAM);
    expect(screen.getByRole("button", { name: "Copy code" })).toHaveTextContent("Copied");
  });

  it("copies the SAME raw fence source in source mode", async () => {
    const user = userEvent.setup();
    const writeText = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue();
    succeed();
    render(<MarkdownAnswer text={FENCE} />);
    await waitFor(() => expect(mockRender).toHaveBeenCalled());

    await user.click(screen.getByRole("button", { name: "Show source" }));
    await user.click(screen.getByRole("button", { name: "Copy code" }));
    expect(writeText).toHaveBeenCalledWith(DIAGRAM);
  });

  it("labels the block with its mermaid language, like every other fence", async () => {
    succeed();
    render(<MarkdownAnswer text={FENCE} />);
    await waitFor(() => expect(mockRender).toHaveBeenCalled());
    expect(screen.getByText("mermaid")).toBeInTheDocument();
  });
});

describe("MermaidBlock — a fence that arrives by streaming deltas", () => {
  it("shows the growing source without claiming failure, then renders once it settles", async () => {
    // An answer streams token by token, so react-markdown hands the mermaid branch a
    // GROWING fence: remark treats the unterminated ``` as a code block with whatever
    // has arrived. Each partial is unparseable, so rendering every delta would flash
    // "could not be rendered" over a diagram that is merely unfinished — and queue one
    // full mermaid.run() per token. The deltas must coalesce into ONE render.
    succeed();
    const { rerender } = render(<MarkdownAnswer text={"```mermaid\ngraph LR;"} />);
    // Mid-stream: the partial source is on screen, escaped, with no failure claim.
    expect(target().textContent).toBe("graph LR;");
    expect(screen.queryByText(/could not be rendered/)).not.toBeInTheDocument();

    rerender(<MarkdownAnswer text={"```mermaid\ngraph LR;\n  a-->b"} />);
    expect(target().textContent).toBe("graph LR;\n  a-->b");
    rerender(<MarkdownAnswer text={FENCE} />);

    await waitFor(() => expect(target().querySelector("svg")).not.toBeNull());
    // One render for the settled text — not one per delta — and one SVG, not a stack.
    expect(mockRender).toHaveBeenCalledTimes(1);
    expect(target().querySelectorAll("svg")).toHaveLength(1);
  });

  it("keeps the newest source when an EARLIER attempt resolves last", async () => {
    // A genuinely out-of-order resolution: the first attempt is held open, a newer
    // fence arrives, and only then does the stale attempt resolve. Its continuation is
    // cancelled, so it must neither paint nor overwrite the newer source.
    let releaseFirst = () => {};
    mockRender.mockImplementationOnce(
      () => new Promise<void>((resolve) => (releaseFirst = () => resolve())),
    );
    const { rerender } = render(<MarkdownAnswer text={FENCE} />);
    await waitFor(() => expect(mockRender).toHaveBeenCalledTimes(1));

    const newer = "graph LR;\n  a-->b-->c";
    rerender(<MarkdownAnswer text={"```mermaid\n" + newer + "\n```\n"} />);
    expect(target().textContent).toBe(newer);

    releaseFirst();
    await waitFor(() => expect(mockRender).toHaveBeenCalledTimes(2));
    expect(target().textContent).toBe(newer);
    expect(target().querySelector("svg")).toBeNull();
  });
});

describe("MermaidBlock — theme tracking (ADR-44)", () => {
  /** The app's theme toggle, so the diagram is re-rendered with the new
   *  themeVariables — mirrors the wiki reader's theme-toggle test. */
  function ToggleTheme() {
    const { toggleTheme } = useTheme();
    return (
      <button type="button" onClick={toggleTheme}>
        Toggle theme
      </button>
    );
  }

  it("re-renders the diagram through the seam when the theme flips", async () => {
    const user = userEvent.setup();
    succeed();
    render(
      <ThemeProvider>
        <ToggleTheme />
        <MarkdownAnswer text={FENCE} />
      </ThemeProvider>,
    );
    await waitFor(() => expect(mockRender).toHaveBeenCalledTimes(1));

    await user.click(screen.getByRole("button", { name: "Toggle theme" }));
    await waitFor(() => expect(mockRender).toHaveBeenCalledTimes(2));
    // Re-rendered cleanly, not stacked on the previous SVG.
    expect(target().querySelectorAll("svg")).toHaveLength(1);
  });

});

// `MermaidBlock` reads the theme context OPTIONALLY — `useTheme()` would throw
// outside a provider, and a provider-less mount must not take the answer down with
// it. Every test in this file except the theme-toggle one above mounts without a
// ThemeProvider, so that guard is already witnessed throughout.

describe("CopyControl — the Copy affordance both block kinds share", () => {
  it("returns to 'Copy' after the acknowledgement window", async () => {
    const user = userEvent.setup();
    vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue();
    render(<MarkdownAnswer text={"```rust\nfn main() {}\n```\n"} />);

    const button = screen.getByRole("button", { name: "Copy code" });
    await user.click(button);
    expect(button.textContent).toBe("Copied");
    // The acknowledgement is transient (1500 ms) and must clear itself, or the
    // control claims forever that its CURRENT text is on the clipboard. Exact text,
    // not `toHaveTextContent`, which would match "Copied" as a substring of itself.
    await waitFor(() => expect(button.textContent).toBe("Copy"), { timeout: 3000 });
  });

  it("stays quiet when the clipboard is blocked", async () => {
    const user = userEvent.setup();
    vi.spyOn(navigator.clipboard, "writeText").mockRejectedValue(new Error("blocked"));
    render(<MarkdownAnswer text={"```rust\nfn main() {}\n```\n"} />);

    const button = screen.getByRole("button", { name: "Copy code" });
    await user.click(button);
    // A blocked clipboard is not the user's problem to solve — no throw, and no
    // false "Copied" claim for something that never reached the clipboard.
    expect(button.textContent).toBe("Copy");
  });
});

describe("MarkdownAnswer — non-mermaid blocks are untouched", () => {
  it("keeps a plain fenced block as source with its unchanged Copy control", async () => {
    const user = userEvent.setup();
    const writeText = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue();
    render(<MarkdownAnswer text={"```rust\nfn main() {}\n```\n"} />);

    expect(screen.getByText("rust")).toBeInTheDocument();
    expect(document.querySelector(".mermaid")).toBeNull();
    // No diagram in the answer means the vendored bundle is never even asked for.
    expect(mockRender).not.toHaveBeenCalled();
    // No diagram-only affordances leak onto an ordinary block.
    expect(screen.queryByRole("button", { name: "Show source" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Zoom in" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Copy code" }));
    expect(writeText).toHaveBeenCalledWith("fn main() {}");
  });

  it("does not treat an UNLABELLED fence of diagram-ish text as a diagram", () => {
    render(<MarkdownAnswer text={"```\ngraph LR;\n  a-->b\n```\n"} />);
    expect(document.querySelector(".mermaid")).toBeNull();
    expect(mockRender).not.toHaveBeenCalled();
    expect(screen.getByText("code")).toBeInTheDocument();
  });

  it("matches the mermaid language case-insensitively", async () => {
    succeed();
    render(<MarkdownAnswer text={"```Mermaid\ngraph LR;\n  a-->b\n```\n"} />);
    await waitFor(() => expect(mockRender).toHaveBeenCalledTimes(1));
  });

  it("keeps inline code a bare <code>, never a diagram", () => {
    render(<MarkdownAnswer text={"use `mermaid` for diagrams"} />);
    expect(document.querySelector(".mermaid")).toBeNull();
    expect(screen.getByText("mermaid").tagName).toBe("CODE");
  });
});
