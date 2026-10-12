import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App.tsx";
import { bootExtensions } from "./extensions/boot.ts";
import { ThemeProvider } from "./theme/ThemeProvider.tsx";
// Global layers, in cascade order: design tokens first (custom properties), then
// the element/reset base built on them. Vite extracts both to external hashed CSS
// referenced via <link> — never an inline <style> (self-only CSP; ADR-44 bans
// runtime CSS-in-JS).
import "./styles/tokens.css";
import "./styles/base.css";

// SPA entry (CR-049, FR-UI-22). Mounts the React tree into the #root element of
// the served shell. The design-system foundation (tokens, theming, component
// library) re-skins the S-185 shell as its first consumer (S-193).
// The one read of the active extensions (S-642, FR-UI-51), started before the first
// render and not awaited: the shell paints at once and holds back only a route an
// extension owns until the answer is in (`extensionDecisionPending`).
void bootExtensions();

const root = document.getElementById("root");
if (root) {
  createRoot(root).render(
    <StrictMode>
      <ThemeProvider>
        <App />
      </ThemeProvider>
    </StrictMode>,
  );
}
