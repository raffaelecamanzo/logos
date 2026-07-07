// Logos wiki Mermaid renderer bootstrap — Project Apache-2.0 (see web/assets/VENDOR.md).
//
// Renders every `.mermaid` block — the native dependency diagram (FR-WK-10) and
// any agent-prose ```mermaid fence (FR-WK-15) — into an inline SVG, fully
// offline. The vendored mermaid bundle is served same-origin and this script
// names no external origin, so the self-only CSP (`script-src 'self'`) and the
// zero-egress posture (NFR-SE-01) hold unchanged — no CDN, no outbound fetch.
//
// Progressive enhancement only: this file loads `defer`-ed after the bundle, so
// with scripting disabled (or the bundle absent) the `.mermaid` element keeps its
// readable diagram source visible — never a blank (FR-WK-15, NFR-CC-04).
//
// The diagram's *theme* (node/edge/label colors) is supplied by the same-origin
// logos.css `.mermaid` rules, not Mermaid's own injected `<style>` block: under
// the self-only CSP (`style-src` falls back to `'self'`, no `unsafe-inline`) that
// injected block is inert, so styling a diagram cleanly means styling Mermaid's
// stable SVG classes from the embedded stylesheet. Mermaid still lays the SVG out
// (shapes, positions, `<text>` labels), which is what makes the diagram visual.
//
// Render fidelity (FR-WK-15, CR-034): Mermaid sizes each node box to fit its
// label using a *layout* font, then logos.css repaints `.mermaid text` in the
// site sans font at 0.85rem. The injected `<style>` that would normally keep the
// two in sync is stripped by the self-only CSP, so a default-font layout paired
// with a different painted font makes labels overflow their boxes / start
// mid-box. We close the gap by handing Mermaid the SAME family + size the CSS
// paints with — read from the very `--font-sans` custom property logos.css uses,
// so there is one source of truth and no drift. `.mermaid text` is 0.85rem; we
// resolve that against the root font-size into the px Mermaid's layout expects.
(function () {
  "use strict";
  var mermaid = window.mermaid;
  if (!mermaid || typeof mermaid.initialize !== "function") {
    return;
  }
  // Keep this in lockstep with the `.mermaid text` `font-size` in logos.css; the
  // `css_mermaid_label_font_size_matches_init` asset test fails if they diverge.
  var LABEL_FONT_REM = 0.85;
  var fontFamily = "";
  var labelFontPx = 0;
  try {
    var rootStyle = window.getComputedStyle(document.documentElement);
    // The exact family stack logos.css paints `.mermaid text` with (`var(--font-sans)`).
    fontFamily = (rootStyle.getPropertyValue("--font-sans") || "").trim();
    var rootFontPx = parseFloat(rootStyle.fontSize);
    if (rootFontPx > 0) {
      labelFontPx = LABEL_FONT_REM * rootFontPx;
    }
  } catch (e) {
    // getComputedStyle should never throw in a browser; if it does, fall through
    // with empty values so initialize keeps Mermaid's own defaults (still renders).
  }
  try {
    var config = {
      // We drive rendering explicitly below; never auto-run on load.
      startOnLoad: false,
      // Treat diagram-authored text as untrusted: escape any HTML and disable
      // interaction/script eval. ('strict' renders in place — it does NOT iframe
      // the diagram the way 'sandbox' would, which the self-only CSP would block.)
      securityLevel: "strict",
      // Pure SVG `<text>` labels rather than `<foreignObject>` HTML labels: the
      // latter embed XHTML styled via CSS that the self-only CSP would strip,
      // leaving labels invisible. SVG text is positioned by attributes and styled
      // by the same-origin `.mermaid` rules, so it stays legible under the CSP.
      htmlLabels: false,
      flowchart: { htmlLabels: false, useMaxWidth: true },
      theme: "neutral",
    };
    // Align the layout font with the painted font so boxes are sized for the text
    // they actually show. Mermaid measures label widths with the top-level
    // `fontFamily`/`fontSize` config (independent of the CSP-stripped injected
    // style), and `themeVariables` keeps any class-driven sizing consistent. The
    // two keys take different forms by Mermaid's API contract: top-level `fontSize`
    // is a bare number (px), `themeVariables.fontSize` is a CSS length string.
    // Each font property is guarded independently (either may be unavailable).
    var themeVariables = {};
    if (fontFamily) {
      config.fontFamily = fontFamily;
      themeVariables.fontFamily = fontFamily;
    }
    if (labelFontPx > 0) {
      config.fontSize = labelFontPx;
      themeVariables.fontSize = labelFontPx + "px";
    }
    if (Object.keys(themeVariables).length > 0) {
      config.themeVariables = themeVariables;
    }
    mermaid.initialize(config);
  } catch (e) {
    if (window.console && console.warn) {
      console.warn("mermaid: initialize failed; diagram source left visible", e);
    }
    return;
  }
  // `run` is deferred, so the DOM is already parsed: it finds every `.mermaid`
  // node and swaps its source for the rendered diagram. A parse error renders an
  // in-place error diagram (still not a blank); guard the promise so a failure
  // never escapes the bootstrap and breaks the page.
  try {
    var done = mermaid.run({ querySelector: ".mermaid" });
    if (done && typeof done.catch === "function") {
      done.catch(function (e) {
        if (window.console && console.warn) {
          console.warn("mermaid: a diagram failed to render", e);
        }
      });
    }
  } catch (e) {
    if (window.console && console.warn) {
      console.warn("mermaid: run failed; diagram source left visible", e);
    }
  }
})();
