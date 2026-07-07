/*
 * Theme bootstrap — no-flash theme application (S-193, CR-050, FR-UI-23, ADR-44).
 * Project MIT.
 *
 * Applies the user's PERSISTED theme choice to <html data-theme> synchronously,
 * BEFORE first paint, so a returning user whose saved choice differs from their OS
 * `prefers-color-scheme` never sees a flash of the wrong theme.
 *
 * Why an external classic script (not an inline one, not the React bundle):
 *   - The self-only CSP forbids inline <script> (`unsafe-inline`), so the usual
 *     inline head snippet is banned. An EXTERNAL same-origin classic script
 *     (`<script src>`) is CSP-clean and still runs synchronously during <head>
 *     parse — before the body paints.
 *   - The React entry is a deferred ES module; it runs AFTER first paint, too late
 *     to prevent a flash.
 *
 * When NO choice is stored, this no-ops and leaves <html> without a data-theme
 * attribute — the CSS `prefers-color-scheme` rule then resolves the first-visit
 * default (dark, or light on a light-OS) with zero JS. The React ThemeProvider
 * owns runtime toggling and persistence from here on.
 *
 * Names no external origin; uses no eval. Mirrors web/ui/src/theme/theme.ts.
 */
(function () {
  try {
    var choice = window.localStorage.getItem("logos-theme");
    if (choice === "light" || choice === "dark") {
      document.documentElement.setAttribute("data-theme", choice);
    }
  } catch (e) {
    /* localStorage can throw (private mode / disabled) — fall back to the CSS
       prefers-color-scheme default rather than failing the page load. */
  }
})();
