/*
 * nav-progress.js — in-flight navigation & fragment loading affordance.
 *
 * Authored for Logos (Project Apache-2.0). License: see the repository `LICENSE` and
 * the provenance manifest `web/assets/VENDOR.md`. This file references no
 * external origin: it is embedded into the `logos` binary (`include_bytes!` in
 * `web/src/assets.rs`) and served same-origin under the self-only CSP
 * (FR-UI-07, FR-UI-02, NFR-CR-01, ADR-27). `cargo build` is the entire build —
 * no Node toolchain, no bundler.
 *
 * Progressive enhancement ONLY (UAT-UI-01): with this script absent or with
 * JavaScript disabled, navigation behaves exactly as plain links and no bar is
 * ever shown — "the server is the state". When present, it acknowledges a click
 * before its page or fragment arrives by arming, after a short delay, a thin
 * orange top progress bar plus a pending state on the clicked nav item; the
 * affordance clears when the new document loads or the fragment request settles.
 *
 * The ~120ms arming delay is deliberate: an instant or cached navigation
 * replaces the current document before the timer fires, so its bar (which lives
 * in the outgoing document) never flashes. Only a genuinely slow response lets
 * the bar appear. Orange (`--so-orange`) is the sanctioned "in-flight" token,
 * distinct from the red `.is-active` "this is the current page" state.
 */
(function () {
  "use strict";

  // Instant/cached responses settle before this, so the bar never flashes.
  var ARM_DELAY_MS = 120;

  var bar = null; // lazily-created top progress bar element
  var armTimer = 0; // pending setTimeout handle (0 = none)
  var pendingItem = null; // the .nav-item currently marked pending
  var htmxInflight = 0; // count of overlapping htmx fragment requests

  // Create (once) the orange top progress bar. It is injected here — never in
  // the server-rendered markup — so the no-JS page stays byte-identical.
  function ensureBar() {
    if (!bar) {
      bar = document.createElement("div");
      bar.className = "nav-progress";
      bar.setAttribute("role", "presentation");
      bar.setAttribute("aria-hidden", "true");
      document.body.appendChild(bar);
    }
    return bar;
  }

  // The enclosing sidebar nav item of an element, if any (so the pending
  // left-bar lands on the clicked navigation item, not an inner span).
  function navItemOf(el) {
    return el && el.closest ? el.closest(".nav-item") : null;
  }

  // Reveal the affordance: orange top bar + pending nav-item state.
  function show(item) {
    armTimer = 0;
    // Re-trigger the fill animation from the start on each arm.
    var b = ensureBar();
    b.classList.remove("is-active");
    void b.offsetWidth; // reflow so the animation restarts
    b.classList.add("is-active");
    // Move the pending marker to the newly-armed item, never leaving a stale
    // .is-pending on an item an earlier arm targeted.
    if (pendingItem && pendingItem !== item) {
      pendingItem.classList.remove("is-pending");
      pendingItem = null;
    }
    if (item) {
      item.classList.add("is-pending");
      pendingItem = item;
    }
  }

  // Arm the affordance after the delay; a second arm resets the timer.
  function arm(item) {
    if (armTimer) {
      clearTimeout(armTimer);
    }
    armTimer = setTimeout(function () {
      show(item);
    }, ARM_DELAY_MS);
  }

  // Clear everything: cancel a pending arm and hide an active bar / pending item.
  function clear() {
    if (armTimer) {
      clearTimeout(armTimer);
      armTimer = 0;
    }
    if (bar) {
      bar.classList.remove("is-active");
    }
    if (pendingItem) {
      pendingItem.classList.remove("is-pending");
      pendingItem = null;
    }
  }

  // ── Full-page navigation ────────────────────────────────────────────────
  // Arm on a click that will actually leave the current document via a plain
  // link. The outgoing document keeps the bar until the new page replaces it
  // (slow) or unloads first (instant) — no explicit clear needed for full nav.
  document.addEventListener("click", function (e) {
    // Honour modified clicks (new tab/window), non-primary buttons, and any
    // handler that already cancelled the event.
    if (
      e.defaultPrevented ||
      e.button !== 0 ||
      e.metaKey ||
      e.ctrlKey ||
      e.shiftKey ||
      e.altKey
    ) {
      return;
    }
    var a = e.target.closest ? e.target.closest("a[href]") : null;
    if (!a) {
      return;
    }
    // Opens elsewhere, downloads, or is htmx-driven (a fragment, not a nav).
    if (a.target && a.target !== "_self") {
      return;
    }
    if (a.hasAttribute("download") || a.hasAttribute("hx-get") || a.hasAttribute("hx-post")) {
      return;
    }
    var href = a.getAttribute("href");
    if (!href || href.charAt(0) === "#") {
      return; // in-page anchor — no navigation
    }
    var url = new URL(a.href, location.href);
    if (url.origin !== location.origin) {
      return; // same-origin only (defensive — every Logos link is same-origin)
    }
    // A pure same-page hash change is not a navigation.
    if (url.pathname === location.pathname && url.search === location.search && url.hash) {
      return;
    }
    arm(navItemOf(a));
  });

  // A page restored from the back/forward (bfcache) starts visually clean.
  window.addEventListener("pageshow", clear);

  // ── htmx fragment requests (search today; sort/filter/paginate later) ────
  // Count on `htmx:beforeSend`, NOT `htmx:beforeRequest`: htmx fires
  // beforeRequest for every candidate request but only sends — and only later
  // fires the paired `htmx:afterRequest` — for those a beforeRequest handler did
  // not cancel. Incrementing on beforeRequest would leak a count for a cancelled
  // request, leaving htmxInflight stuck above zero and the bar armed for the rest
  // of the page's life. beforeSend↔afterRequest is htmx's guaranteed pair (it
  // fires afterRequest on success and error alike). htmx events bubble to
  // <body>; if htmx is absent these listeners simply never fire.
  document.body.addEventListener("htmx:beforeSend", function (e) {
    htmxInflight++;
    arm(navItemOf(e.target));
  });
  function htmxSettled() {
    if (htmxInflight > 0) {
      htmxInflight--;
    }
    if (htmxInflight === 0) {
      clear();
    }
  }
  document.body.addEventListener("htmx:afterRequest", htmxSettled);
})();
