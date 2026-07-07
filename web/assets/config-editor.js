/*
 * config-editor.js — the Config view's client module (S-099, CR-025, FR-UI-12,
 * BR-35).
 *
 * Authored for Logos (Project MIT). License: see the repository `LICENSE` and
 * the provenance manifest `web/assets/VENDOR.md`. This file references no
 * external origin: it is embedded into the `logos` binary (`include_bytes!` in
 * `web/src/assets.rs`) and served same-origin under the self-only CSP
 * (FR-UI-02, NFR-CR-01, ADR-27). `cargo build` is the entire build.
 *
 * What it does, and why a script is required here at all:
 *  - The self-only CSP carries `form-action 'none'`, so a native <form> POST to
 *    the config-write route is blocked by the browser before it is sent. Saving
 *    therefore goes through `fetch` (an XHR-class request, unaffected by
 *    `form-action`), which lets us attach the per-session intent header the
 *    surface requires (NFR-SE-06, ADR-31). The token is read from the editor
 *    root's `data-intent-token` (embedded by the GET handler), never inlined.
 *  - Typed fields patch their key into the raw-TOML candidate (the authoritative
 *    document that is posted verbatim), so a typed edit and a hand edit both flow
 *    into the one validated document — no second source of truth. A patch bug can
 *    at worst produce a 422 the engine names; it can never cause a partial write.
 *  - A `rules.toml` save is gated behind an explicit confirmation (BR-35); the
 *    engine stamps the provenance comment, and we surface it on success.
 *  - An in-flight affordance (FR-UI-07): the Save button is disabled and a
 *    "Saving…" status shows while the request is outstanding.
 *  - Apply (S-100, FR-UI-13) is the separate, deliberate step that runs the
 *    pipeline. The "Apply & reindex"/"Apply & re-evaluate" button POSTs only
 *    `file=<file>` to `/config/apply` (never `content`), so the engine applies
 *    the *saved* on-disk file: config.toml reconciles the graph, rules.toml
 *    re-evaluates the gate. It shows its own in-flight affordance and renders the
 *    ConfigApplyOutcome (reconciled / reevaluated) — or an explicit error on
 *    failure — into a dedicated panel, never a blank or a stale figure. Save
 *    alone never applies.
 *
 * Progressive enhancement only (UAT-UI-01): with JS disabled the view still
 * renders and reads the current config; only saving — which fundamentally needs
 * the intent header the CSP-blocked native form cannot set — is unavailable.
 *
 * No data is ever assigned through innerHTML: all dynamic text is written with
 * textContent and all structure is built with DOM methods, so a config value or
 * a server error string can never be interpreted as markup.
 */
(function () {
  "use strict";

  var root = document.querySelector(".config-editor");
  if (!root) {
    return;
  }
  var token = root.getAttribute("data-intent-token") || "";
  var headerName = root.getAttribute("data-intent-header") || "x-logos-intent";
  var saveRoute = root.getAttribute("data-save-route") || "/config/save";
  var applyRoute = root.getAttribute("data-apply-route") || "/config/apply";
  var secretRoute = root.getAttribute("data-secret-route") || "/config/secret";

  // ── Raw-TOML candidate access ────────────────────────────────────────────
  function rawFor(file) {
    return root.querySelector('textarea[data-config-raw="' + file + '"]');
  }

  // ── TOML serialisation for the bounded typed-field shapes ────────────────
  // A TOML basic string is a JSON string for our character set (double-quoted,
  // backslash escapes), so JSON.stringify is a correct, dependency-free encoder.
  function tomlString(s) {
    return JSON.stringify(String(s));
  }

  // Serialise a typed field's value to its TOML right-hand side, or null when the
  // field is empty (an optional key that should be removed so it reverts to its
  // default).
  function tomlValue(type, value) {
    if (type === "list") {
      var items = value
        .split("\n")
        .map(function (s) {
          return s.trim();
        })
        .filter(function (s) {
          return s.length > 0;
        });
      // A cleared list field removes the key (reverting to the engine default),
      // consistent with the optional scalars — never `key = []`, which is a
      // distinct, surprising "disable everything" for `languages`/`include`. The
      // explicit empty-array intent stays available by hand in the raw pane.
      if (items.length === 0) {
        return null;
      }
      return "[" + items.map(tomlString).join(", ") + "]";
    }
    if (type === "bool") {
      return value === "true" ? "true" : value === "false" ? "false" : null;
    }
    if (type === "int" || type === "float") {
      return value.trim() === "" ? null : value.trim();
    }
    if (type === "str") {
      // An optional string scalar (the [chat] provider/model fields): a blank
      // field removes the key so it reverts to its default/unset state, just
      // like the optional numbers above; a non-blank value is a quoted TOML
      // basic string. The provider <select> has no blank option, so it always
      // writes one of its two enumerated values.
      return value.trim() === "" ? null : tomlString(value.trim());
    }
    return tomlString(value); // a plain string scalar
  }

  function escapeRe(s) {
    return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  }

  // The [start, end) line range owning `table` ("" = the top-level region above
  // the first table header). `headerIdx` is the table-header line, or -1 when the
  // table is absent and must be created on insert.
  function regionBounds(lines, table) {
    if (table === "") {
      var firstHeader = lines.length;
      for (var i = 0; i < lines.length; i++) {
        if (/^\s*\[/.test(lines[i])) {
          firstHeader = i;
          break;
        }
      }
      return { start: 0, end: firstHeader, headerIdx: -1 };
    }
    var headerRe = new RegExp("^\\s*\\[" + escapeRe(table) + "\\]\\s*(#.*)?$");
    var hdr = -1;
    for (var j = 0; j < lines.length; j++) {
      if (headerRe.test(lines[j])) {
        hdr = j;
        break;
      }
    }
    if (hdr === -1) {
      return { start: lines.length, end: lines.length, headerIdx: -1 };
    }
    var end = lines.length;
    for (var k = hdr + 1; k < lines.length; k++) {
      if (/^\s*\[/.test(lines[k])) {
        end = k;
        break;
      }
    }
    return { start: hdr + 1, end: end, headerIdx: hdr };
  }

  // Patch (replace / insert / remove) `key` in `table` of the raw document text.
  function patch(raw, table, key, type, value) {
    var serialised = tomlValue(type, value);
    var lines = raw.split("\n");
    var region = regionBounds(lines, table);
    var keyRe = new RegExp("^\\s*" + escapeRe(key) + "\\s*=");
    var found = -1;
    for (var i = region.start; i < region.end; i++) {
      if (keyRe.test(lines[i])) {
        found = i;
        break;
      }
    }
    if (serialised === null) {
      if (found >= 0) {
        lines.splice(found, 1);
      }
      return lines.join("\n");
    }
    var newLine = key + " = " + serialised;
    if (found >= 0) {
      lines[found] = newLine;
    } else if (table !== "" && region.headerIdx >= 0) {
      lines.splice(region.start, 0, newLine); // right after the existing header
    } else if (table !== "") {
      lines.push("[" + table + "]"); // create the absent table
      lines.push(newLine);
    } else {
      lines.splice(region.end, 0, newLine); // end of the top-level region
    }
    return lines.join("\n");
  }

  function patchFromField(field) {
    var file = field.getAttribute("data-toml-file");
    var raw = rawFor(file);
    if (!raw) {
      return;
    }
    raw.value = patch(
      raw.value,
      field.getAttribute("data-toml-table") || "",
      field.getAttribute("data-toml-key"),
      field.getAttribute("data-toml-type"),
      field.value
    );
  }

  // ── Result panel rendering (textContent only) ────────────────────────────
  function message(el, kind, text) {
    if (!el) {
      return;
    }
    el.className = "config-result is-" + kind;
    el.textContent = text;
  }

  function clearResult(el) {
    if (el) {
      el.className = "config-result";
      el.textContent = "";
    }
  }

  function renderSuccess(el, file, text) {
    var note = "Saved.";
    try {
      var outcome = JSON.parse(text);
      note = "Saved " + outcome.path + " (" + outcome.bytes_written + " bytes).";
      if (outcome.provenance_stamped) {
        note += " A provenance comment was stamped into the file.";
      }
    } catch (e) {
      // Non-JSON 2xx is unexpected; show the raw text honestly rather than guess.
      note = "Saved: " + text;
    }
    note += " Changes are not applied until you Apply (reindex / re-evaluate).";
    message(el, "ok", note);
  }

  function renderError(el, status, text) {
    var label = status === 422 ? "Validation error" : status === 400 ? "Bad request" : "Save failed (" + status + ")";
    message(el, "error", label + ": " + text);
  }

  // ── Apply outcome rendering (ConfigApplyOutcome, S-097) ──────────────────
  function plural(n, word) {
    return n + " " + word + (n === 1 ? "" : "s");
  }

  // Render a successful apply honestly from the internally-tagged outcome: a
  // `reconciled` outcome (config.toml apply) or a `reevaluated` outcome
  // (rules.toml apply). A degradation (failed files / warnings) downgrades the
  // panel from "ok" to "warn" so it is never reported as a clean success.
  function renderApplyOutcome(el, text) {
    var outcome;
    try {
      outcome = JSON.parse(text);
    } catch (e) {
      // A 2xx that is not the expected JSON: surface the raw text, never guess.
      message(el, "warn", "Applied, but the outcome was not understood: " + text);
      return;
    }
    var note;
    var kind = "ok";
    if (outcome.action === "reconciled") {
      note = "Applied — reconciled " + plural(outcome.reconciled_files, "file") + ".";
      if (outcome.full_index) {
        note += " A full index was performed (the graph had not been indexed yet).";
      }
      note += " " + plural(outcome.unresolved_refs, "unresolved reference") + ".";
      if (outcome.files_failed && outcome.files_failed.length > 0) {
        kind = "warn";
        note += " Could not read/extract: " + outcome.files_failed.join(", ") + ".";
      }
    } else if (outcome.action === "reevaluated") {
      var signal = outcome.signal === null || outcome.signal === undefined ? "n/a" : outcome.signal;
      note =
        "Applied — gate re-evaluated (no reindex). Signal " +
        signal +
        ", " +
        plural(outcome.violations, "violation") +
        ". " +
        outcome.freshness +
        ".";
    } else {
      // An unknown tag is still honest output, not a fabricated success.
      message(el, "warn", "Applied, but the outcome action was not recognised: " + text);
      return;
    }
    if (outcome.warnings && outcome.warnings.length > 0) {
      kind = "warn";
      note += " Warnings: " + outcome.warnings.join("; ") + ".";
    }
    note += " Reload the affected views to see the updated graph / gate.";
    message(el, kind, note);
  }

  function renderApplyError(el, status, text) {
    var label = status === 400 ? "Bad request" : status >= 500 ? "Apply failed (" + status + ")" : "Apply rejected (" + status + ")";
    message(el, "error", label + ": " + text);
  }

  function setBusy(btn, busyEl, busy) {
    if (btn) {
      btn.disabled = busy;
    }
    if (busyEl) {
      busyEl.hidden = !busy;
    }
    root.setAttribute("aria-busy", busy ? "true" : "false");
  }

  // ── Save ──────────────────────────────────────────────────────────────────
  function save(file) {
    var resultEl = root.querySelector('[data-config-result="' + file + '"]');
    var busyEl = root.querySelector('[data-config-busy="' + file + '"]');
    var btn = root.querySelector('[data-config-save="' + file + '"]');

    // rules.toml confirmation gate (BR-35): no confirmation ⇒ no POST, no write.
    if (file === "rules") {
      var confirmEl = root.querySelector('[data-config-confirm="rules"]');
      if (!confirmEl || !confirmEl.checked) {
        message(
          resultEl,
          "warn",
          "Confirm the change before saving rules.toml — this changes what the gate enforces."
        );
        return;
      }
    }

    var raw = rawFor(file);
    if (!raw) {
      return;
    }
    var body = "file=" + encodeURIComponent(file) + "&content=" + encodeURIComponent(raw.value);
    var headers = { "Content-Type": "application/x-www-form-urlencoded" };
    headers[headerName] = token;

    setBusy(btn, busyEl, true);
    clearResult(resultEl);

    fetch(saveRoute, {
      method: "POST",
      headers: headers,
      body: body,
      credentials: "same-origin"
    })
      .then(function (res) {
        return res.text().then(function (text) {
          return { ok: res.ok, status: res.status, text: text };
        });
      })
      .then(function (r) {
        setBusy(btn, busyEl, false);
        if (r.ok) {
          renderSuccess(resultEl, file, r.text);
        } else {
          renderError(resultEl, r.status, r.text);
        }
      })
      .catch(function (err) {
        setBusy(btn, busyEl, false);
        message(resultEl, "error", "Save failed: " + (err && err.message ? err.message : "request error"));
      });
  }

  // ── Apply (FR-UI-13, FR-UI-07) ───────────────────────────────────────────
  // The deliberate, separate step that runs the pipeline over the *saved* file.
  // Unlike save, it posts only `file=<file>` (no `content`); the engine applies
  // what is on disk. Save is never coupled to Apply.
  function apply(file) {
    var resultEl = root.querySelector('[data-config-apply-result="' + file + '"]');
    var busyEl = root.querySelector('[data-config-apply-busy="' + file + '"]');
    var btn = root.querySelector('[data-config-apply="' + file + '"]');

    var body = "file=" + encodeURIComponent(file);
    var headers = { "Content-Type": "application/x-www-form-urlencoded" };
    headers[headerName] = token;

    setBusy(btn, busyEl, true);
    clearResult(resultEl);

    fetch(applyRoute, {
      method: "POST",
      headers: headers,
      body: body,
      credentials: "same-origin"
    })
      .then(function (res) {
        return res.text().then(function (text) {
          return { ok: res.ok, status: res.status, text: text };
        });
      })
      .then(function (r) {
        setBusy(btn, busyEl, false);
        if (r.ok) {
          renderApplyOutcome(resultEl, r.text);
        } else {
          renderApplyError(resultEl, r.status, r.text);
        }
      })
      .catch(function (err) {
        setBusy(btn, busyEl, false);
        message(resultEl, "error", "Apply failed: " + (err && err.message ? err.message : "request error"));
      });
  }

  // ── Chat API key (S-169, FR-CF-06, NFR-SE-07) ────────────────────────────
  // The key lives in the gitignored secrets.toml, so it has its own write route
  // and is write-only: the input is never pre-filled (the browser never receives
  // the stored key), and the response carries only the masked new state
  // (presence + last-4) — we never render the secret. Saving an empty value
  // clears the key. After a successful write the input is cleared so the typed
  // secret does not linger in the DOM.
  function renderSecretSuccess(el, text) {
    var note = "Key saved.";
    try {
      var outcome = JSON.parse(text);
      if (outcome.chat_key && outcome.chat_key.present) {
        var tail = outcome.chat_key.last4 ? " (ends …" + outcome.chat_key.last4 + ")" : "";
        note = "Key saved" + tail + ". It is stored in " + outcome.path + " and never echoed.";
      } else {
        note = "Key cleared. " + outcome.path + " no longer holds a chat key.";
      }
    } catch (e) {
      // Never echo the raw response body on the secret path: a non-JSON 2xx is
      // unexpected, and the body could in principle carry key material if the
      // server contract ever changed. Show a fixed string, not `text`
      // (NFR-SE-07 — the key/raw body is never rendered).
      note = "Key saved (unexpected response format).";
    }
    message(el, "ok", note);
  }

  function saveSecret() {
    var input = root.querySelector("[data-config-secret-input]");
    var resultEl = root.querySelector("[data-config-secret-result]");
    var busyEl = root.querySelector("[data-config-secret-busy]");
    var btn = root.querySelector("[data-config-secret-save]");
    if (!input) {
      return;
    }
    var body = "api_key=" + encodeURIComponent(input.value);
    var headers = { "Content-Type": "application/x-www-form-urlencoded" };
    headers[headerName] = token;

    setBusy(btn, busyEl, true);
    clearResult(resultEl);

    fetch(secretRoute, {
      method: "POST",
      headers: headers,
      body: body,
      credentials: "same-origin"
    })
      .then(function (res) {
        return res.text().then(function (text) {
          return { ok: res.ok, status: res.status, text: text };
        });
      })
      .then(function (r) {
        setBusy(btn, busyEl, false);
        if (r.ok) {
          // Never leave the typed secret in the DOM after it is persisted.
          input.value = "";
          renderSecretSuccess(resultEl, r.text);
        } else {
          renderError(resultEl, r.status, r.text);
        }
      })
      .catch(function (err) {
        setBusy(btn, busyEl, false);
        message(resultEl, "error", "Save failed: " + (err && err.message ? err.message : "request error"));
      });
  }

  // ── Wiring ──────────────────────────────────────────────────────────────
  // Typed fields keep the raw candidate current as they change, so the textarea
  // a user can also hand-edit is always the single posted source of truth.
  var fields = root.querySelectorAll("[data-toml-key]");
  for (var f = 0; f < fields.length; f++) {
    var ev = fields[f].tagName === "SELECT" ? "change" : "input";
    fields[f].addEventListener(ev, function (e) {
      patchFromField(e.target);
    });
  }

  var buttons = root.querySelectorAll("[data-config-save]");
  for (var b = 0; b < buttons.length; b++) {
    buttons[b].addEventListener("click", function (e) {
      save(e.target.getAttribute("data-config-save"));
    });
  }

  var applyButtons = root.querySelectorAll("[data-config-apply]");
  for (var a = 0; a < applyButtons.length; a++) {
    applyButtons[a].addEventListener("click", function (e) {
      apply(e.target.getAttribute("data-config-apply"));
    });
  }

  var secretBtn = root.querySelector("[data-config-secret-save]");
  if (secretBtn) {
    secretBtn.addEventListener("click", function () {
      saveSecret();
    });
  }
})();
