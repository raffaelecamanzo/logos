// The widget harness page (S-611): one WidgetStack of widgets in each shape the
// frame takes — a figure with evidence, an act action with a badge, an absence,
// and copy only — rendered from the fixture catalogue.
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { Badge } from "../../src/components/Badge.tsx";
import { Widget } from "../../src/components/Widget.tsx";
import { WidgetStack } from "../../src/components/WidgetStack.tsx";
import { coverage, observe, thresholds } from "../../src/copy/fixture.copy.ts";
import "../../src/styles/tokens.css";
import "../../src/styles/base.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <main>
      <WidgetStack>
        <Widget title="Resolved calls" copy={observe} figure={<span>12 of 40</span>}>
          <ul>
            <li>api → web: 7 calls</li>
            <li>web → api: 5 calls</li>
          </ul>
        </Widget>
        <Widget title="Thresholds" badge={<Badge tone="red">fail</Badge>} copy={thresholds} state={{ breached: 2 }} figure="2 of 9 measures" />
        <Widget title="Coverage" copy={coverage} state={{ ingested: false }} absence="No coverage ingested yet." />
        <Widget title="Copy only" copy={observe} />
      </WidgetStack>
    </main>
  </StrictMode>,
);
