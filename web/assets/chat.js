// Chat view client (S-171, CR-045/CR-046, FR-UI-18/19/20).
//
// Project MIT. Authored, same-origin, embedded — names NO external origin
// (NFR-SE-01): the only network call is a `fetch` to the same-origin,
// intent-guarded `POST /chat` (a turn) and `POST /chat/clear` (Clear-history).
//
// Progressive enhancement (FR-UI-19): the composer is a real `<form
// method="post" action="/chat">`. With JS this module intercepts submit and
// streams the turn over Server-Sent Events — rendering the planner's plan, the
// subagent-activity chips, and the answer as events arrive — instead of the
// buffered no-JS reload. The first outbound call is gated behind an explicit
// consent acknowledgement (NFR-SE-07). The streaming request rides the `POST`
// (not a `GET` EventSource) precisely so it can carry the per-session intent
// header a cross-origin page cannot forge (NFR-SE-06).
"use strict";

(function () {
  var form = document.querySelector("[data-chat-form]");
  if (!form) {
    return; // configure-first / unconfigured: no composer to enhance.
  }

  var intentToken = form.getAttribute("data-intent-token") || "";
  var intentHeader = form.getAttribute("data-intent-header") || "x-logos-intent";
  var chatRoute = form.getAttribute("data-chat-route") || "/chat";
  var clearRoute = form.getAttribute("data-chat-clear-route") || "/chat/clear";

  var log = document.querySelector("[data-chat-log]");
  var input = form.querySelector("textarea[name='q']");
  var sendBtn = form.querySelector("[data-chat-send]");
  var busy = form.querySelector("[data-chat-busy]");
  var consent = document.querySelector("[data-chat-consent]");
  var acceptBtn = document.querySelector("[data-chat-consent-accept]");
  var clearBtn = document.querySelector("[data-chat-clear]");
  var clearResult = document.querySelector("[data-chat-clear-result]");

  // ── Consent gate (NFR-SE-07) ───────────────────────────────────────────────
  // The first outbound call is gated behind the explicit "Start chatting"
  // acknowledgement; the decision is remembered client-side so a returning user
  // is not re-prompted, but the disclosure is always rendered (no-JS sees it).
  var CONSENT_KEY = "logos.chat.consent";

  function hasConsent() {
    try {
      return window.localStorage.getItem(CONSENT_KEY) === "1";
    } catch (e) {
      return false; // storage blocked → re-ask each load (fail safe, not open).
    }
  }

  function rememberConsent() {
    try {
      window.localStorage.setItem(CONSENT_KEY, "1");
    } catch (e) {
      /* non-fatal: consent holds for this page even if it cannot persist */
    }
  }

  function setComposerEnabled(enabled) {
    if (input) {
      input.disabled = !enabled;
    }
    if (sendBtn) {
      sendBtn.disabled = !enabled;
    }
  }

  function applyConsentState() {
    if (hasConsent()) {
      if (consent) {
        consent.classList.add("is-consented");
        consent.setAttribute("hidden", "");
      }
      setComposerEnabled(true);
    } else {
      setComposerEnabled(false); // gate: no turn until acknowledged.
    }
  }

  if (acceptBtn) {
    acceptBtn.addEventListener("click", function () {
      rememberConsent();
      applyConsentState();
      if (input) {
        input.focus();
      }
    });
  }
  applyConsentState();

  // ── DOM helpers (build with textContent — never innerHTML for turn data) ─────
  function el(tag, cls, text) {
    var node = document.createElement(tag);
    if (cls) {
      node.className = cls;
    }
    if (text !== undefined && text !== null) {
      node.textContent = text;
    }
    return node;
  }

  function clearChildren(node) {
    while (node && node.firstChild) {
      node.removeChild(node.firstChild);
    }
  }

  function appendToLog(node) {
    if (!log) {
      return;
    }
    var empty = log.querySelector(".chat-empty");
    if (empty) {
      empty.remove();
    }
    log.appendChild(node);
    log.scrollTop = log.scrollHeight;
  }

  // Map a wire role (StepRole serde name) to its display label.
  var ROLE_LABELS = {
    graph_navigator: "Graph-Navigator",
    governance_analyst: "Governance-Analyst",
    source_reader: "Source-Reader",
    synthesizer: "Synthesizer",
  };
  function roleLabel(role) {
    return ROLE_LABELS[role] || role || "subagent";
  }

  // Map a BudgetBound (serde-tagged on "bound") to an honest, named halt note.
  function boundNote(bound) {
    if (!bound) {
      return "the turn halted at a budget bound";
    }
    if (bound.bound === "global_tool_calls") {
      return "halted: the global per-turn tool-call ceiling was reached (" + bound.limit + " calls)";
    }
    if (bound.bound === "subagent_tool_calls") {
      return "halted: a subagent reached its per-subagent tool-call cap (" + bound.limit + " calls)";
    }
    if (bound.bound === "replans") {
      return "halted: the planner reached the max-replans bound (" + bound.limit + " replans)";
    }
    return "the turn halted at a budget bound";
  }

  // ── A single turn's render surface ──────────────────────────────────────────
  function newAssistantTurn() {
    var card = el("div", "chat-msg chat-assistant");
    var plan = el("ol", "chat-plan");
    plan.hidden = true;
    var chips = el("div", "chat-chips");
    var answer = el("div", "chat-answer");
    card.appendChild(plan);
    card.appendChild(chips);
    card.appendChild(answer);
    appendToLog(card);
    // `answerNode` is the live answer paragraph the Synthesizer's tokens stream
    // into; it is created on the first `answer_delta` and finalised by
    // `final_answer` (FR-UI-19).
    return { card: card, plan: plan, chips: chips, answer: answer, chipByIndex: {}, answerNode: null };
  }

  function renderPlan(turn, data) {
    if (!data || !data.steps) {
      return;
    }
    clearChildren(turn.plan);
    turn.plan.hidden = false;
    var caption = el("li", "chat-plan-caption", data.round > 0 ? "Revised plan" : "Plan");
    turn.plan.appendChild(caption);
    for (var i = 0; i < data.steps.length; i++) {
      var step = data.steps[i];
      turn.plan.appendChild(el("li", "chat-plan-step", roleLabel(step.role) + ": " + (step.instruction || "")));
    }
  }

  function renderStepStarted(turn, data) {
    var chip = el("span", "chat-chip is-running");
    var instruction = data.instruction ? " " + data.instruction : "";
    chip.textContent = "▸ " + roleLabel(data.role) + ":" + instruction;
    turn.chips.appendChild(chip);
    turn.chipByIndex[data.index] = chip;
  }

  function renderStepObserved(turn, data) {
    var chip = turn.chipByIndex[data.index];
    if (chip) {
      chip.classList.remove("is-running");
      chip.classList.add("is-done");
      if (data.summary) {
        chip.title = data.summary;
      }
    }
  }

  // The Synthesizer streams the answer token by token (FR-UI-19). Append each
  // delta to the live answer paragraph (created on the first one), with the
  // `is-streaming` marker driving the typewriter caret until `final_answer`
  // reconciles it. textContent only — turn data never becomes HTML.
  function renderAnswerDelta(turn, data) {
    if (!data || typeof data.delta !== "string") {
      return;
    }
    if (!turn.answerNode) {
      turn.answerNode = el("p", "chat-final is-streaming");
      turn.answer.appendChild(turn.answerNode);
    }
    turn.answerNode.textContent += data.delta;
    if (log) {
      log.scrollTop = log.scrollHeight;
    }
  }

  // The turn's authoritative final text. When tokens streamed, reconcile the live
  // paragraph to it and drop the streaming marker; otherwise (event-level only)
  // append the full answer in one block.
  function renderFinalAnswer(turn, data) {
    var answer = (data && data.answer) || "";
    if (turn.answerNode) {
      if (answer) {
        turn.answerNode.textContent = answer;
      }
      turn.answerNode.classList.remove("is-streaming");
      return;
    }
    turn.answer.appendChild(el("p", "chat-final", answer));
  }

  // ── SSE over fetch (the turn) ───────────────────────────────────────────────
  function dispatchEvent(turn, name, dataText) {
    if (name === "error") {
      // The error event carries a plain-text message, not JSON (NFR-CC-04).
      turn.answer.appendChild(el("p", "chat-error", dataText));
      return;
    }
    var data;
    try {
      data = JSON.parse(dataText);
    } catch (e) {
      return; // a malformed frame is dropped rather than guessed at.
    }
    // Flat dispatch (independent `if`/`return`, not an `else if` chain) so the
    // function stays within the max-nesting bound the gate enforces.
    if (name === "plan") {
      renderPlan(turn, data);
      return;
    }
    if (name === "step_started") {
      renderStepStarted(turn, data);
      return;
    }
    if (name === "step_observed") {
      renderStepObserved(turn, data);
      return;
    }
    if (name === "halted") {
      turn.answer.appendChild(el("p", "chat-halt", boundNote(data.bound)));
      return;
    }
    if (name === "answer_delta") {
      renderAnswerDelta(turn, data);
      return;
    }
    if (name === "final_answer") {
      renderFinalAnswer(turn, data);
    }
  }

  // Parse one SSE block (a run of lines up to a blank line) into {event, data}.
  function parseBlock(turn, block) {
    var lines = block.split("\n");
    var name = "message";
    var dataParts = [];
    for (var i = 0; i < lines.length; i++) {
      var line = lines[i];
      if (!line || line.charAt(0) === ":") {
        continue; // blank or keep-alive comment.
      }
      if (line.indexOf("event:") === 0) {
        name = line.slice(6).trim();
      } else if (line.indexOf("data:") === 0) {
        dataParts.push(line.slice(5).replace(/^ /, ""));
      }
    }
    if (dataParts.length > 0) {
      dispatchEvent(turn, name, dataParts.join("\n"));
    }
  }

  function setBusy(on) {
    if (busy) {
      busy.hidden = !on;
    }
    if (sendBtn) {
      sendBtn.disabled = on;
    }
    if (log) {
      log.setAttribute("aria-busy", on ? "true" : "false");
    }
  }

  async function streamTurn(question) {
    var turn = newAssistantTurn();
    setBusy(true);
    try {
      var headers = { "Content-Type": "application/x-www-form-urlencoded", Accept: "text/event-stream" };
      headers[intentHeader] = intentToken;
      var resp = await fetch(chatRoute, {
        method: "POST",
        headers: headers,
        body: "q=" + encodeURIComponent(question),
      });
      if (!resp.ok || !resp.body) {
        turn.answer.appendChild(el("p", "chat-error", "The chat turn could not start (status " + resp.status + ")."));
        return;
      }
      var reader = resp.body.getReader();
      var decoder = new TextDecoder();
      var buffer = "";
      for (;;) {
        var chunk = await reader.read();
        if (chunk.done) {
          break;
        }
        buffer += decoder.decode(chunk.value, { stream: true });
        var sep;
        while ((sep = buffer.indexOf("\n\n")) !== -1) {
          var block = buffer.slice(0, sep);
          buffer = buffer.slice(sep + 2);
          parseBlock(turn, block);
        }
      }
      buffer += decoder.decode(); // flush any partial multi-byte char the streaming decoder held
      if (buffer.trim()) {
        parseBlock(turn, buffer);
      }
      // Honest states (NFR-CC-04): a cleanly-closed stream that produced no
      // answer / halt / error is surfaced, never left as a silently empty turn.
      if (!turn.answer.firstChild) {
        turn.answer.appendChild(
          el("p", "chat-error", "The turn ended without an answer — the connection may have closed early.")
        );
      }
    } catch (e) {
      turn.answer.appendChild(el("p", "chat-error", "The chat turn failed: " + (e && e.message ? e.message : e)));
    } finally {
      setBusy(false);
    }
  }

  form.addEventListener("submit", function (event) {
    event.preventDefault();
    if (!hasConsent()) {
      applyConsentState();
      return; // gate: acknowledge consent first.
    }
    var question = input ? input.value.trim() : "";
    if (!question) {
      return;
    }
    appendToLog(el("div", "chat-msg chat-user", question));
    if (input) {
      input.value = "";
    }
    streamTurn(question);
  });

  // ── Clear-history (FR-UI-20) ────────────────────────────────────────────────
  if (clearBtn) {
    clearBtn.addEventListener("click", async function () {
      if (!window.confirm("Clear all chat history and its memory? This cannot be undone.")) {
        return;
      }
      clearBtn.disabled = true;
      if (clearResult) {
        clearResult.textContent = "Clearing…";
      }
      try {
        var headers = {};
        headers[intentHeader] = intentToken;
        var resp = await fetch(clearRoute, { method: "POST", headers: headers });
        if (!resp.ok) {
          if (clearResult) {
            clearResult.textContent = "Could not clear history (status " + resp.status + ").";
          }
          return;
        }
        if (log) {
          clearChildren(log);
          appendToLog(el("p", "chat-empty muted", "History cleared. Ask a question to start a new turn."));
        }
        if (clearResult) {
          clearResult.textContent = "History cleared.";
        }
      } catch (e) {
        if (clearResult) {
          clearResult.textContent = "Could not clear history: " + (e && e.message ? e.message : e);
        }
      } finally {
        clearBtn.disabled = false;
      }
    });
  }
})();
