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

/** Stand in for a SUCCESSFUL bundle render: replace the target's source with an
 *  SVG and set Mermaid's own `data-processed` latch, as the real bundle does. */
function succeed(svg = "<svg><g class='node'></g></svg>") {
  mockRender.mockImplementation((container: HTMLElement) => {
    const el = container.querySelector<HTMLElement>(".mermaid");
    if (el) {
      el.innerHTML = svg;
      el.setAttribute("data-processed", "true");
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
  });
});

describe("MermaidBlock — zoom, toggle, and copy (FR-UI-32)", () => {
  it("zooms in and out along a bounded ladder and resets to 100%", async () => {
    const user = userEvent.setup();
    succeed();
    render(<MarkdownAnswer text={FENCE} />);
    await waitFor(() => expect(mockRender).toHaveBeenCalled());

    expect(screen.getByText("100%")).toBeInTheDocument();
    // Reset is spent at the default step.
    expect(screen.getByRole("button", { name: "Reset zoom" })).toBeDisabled();

    await user.click(screen.getByRole("button", { name: "Zoom in" }));
    expect(screen.getByText("125%")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Zoom out" }));
    await user.click(screen.getByRole("button", { name: "Zoom out" }));
    expect(screen.getByText("80%")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Reset zoom" }));
    expect(screen.getByText("100%")).toBeInTheDocument();
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
  it("shows the partial source, then the diagram once the fence completes", async () => {
    // An answer streams token by token, so react-markdown hands the mermaid branch a
    // GROWING fence: remark treats the unterminated ``` as a code block with whatever
    // has arrived. The viewer must not blank, and must not stack renders.
    succeed();
    const { rerender } = render(<MarkdownAnswer text={"```mermaid\ngraph LR;"} />);
    await waitFor(() => expect(mockRender).toHaveBeenCalledTimes(1));
    expect(target()).toBeInTheDocument();

    rerender(<MarkdownAnswer text={"```mermaid\ngraph LR;\n  a-->b"} />);
    await waitFor(() => expect(mockRender).toHaveBeenCalledTimes(2));

    rerender(<MarkdownAnswer text={FENCE} />);
    await waitFor(() => expect(mockRender).toHaveBeenCalledTimes(3));
    // Settled on the complete diagram — one SVG, not one per delta.
    expect(target().querySelectorAll("svg")).toHaveLength(1);
  });

  it("keeps the newest source when a later delta cannot render", async () => {
    // The seam resolving out of order must not leave a stale diagram claiming to be
    // the answer: the source restored is always the CURRENT fence text.
    succeed();
    const { rerender } = render(<MarkdownAnswer text={FENCE} />);
    await waitFor(() => expect(target().querySelector("svg")).not.toBeNull());

    mockRender.mockImplementation(() => Promise.resolve());
    rerender(<MarkdownAnswer text={"```mermaid\ngraph LR;\n  a-->b-->c"} />);
    await waitFor(() => expect(target().textContent).toBe("graph LR;\n  a-->b-->c"));
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

  it("still renders its diagram with no ThemeProvider above it", async () => {
    // `MermaidBlock` reads the theme context OPTIONALLY — `useTheme()` would throw
    // here, and a provider-less mount must not take the answer down with it.
    succeed();
    render(<MarkdownAnswer text={FENCE} />);
    await waitFor(() => expect(target().querySelector("svg")).not.toBeNull());
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
