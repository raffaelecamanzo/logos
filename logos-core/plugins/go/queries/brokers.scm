; Go message-broker publish/subscribe capture (S-408, capability = "brokers").
;
; Feeds the generic invocation interpreter (extract::broker) → the
; BrokerPublish / BrokerSubscribe fan-out arm ([FR-WS-10], [ADR-54]), exactly as
; the Java and Rust `brokers.scm` do. Capture vocabulary (interpreted by
; `capture_broker_invocations`):
;
;   @broker.publish.topic         — a publish site's topic string literal
;   @broker.subscribe.topic       — a subscribe site's topic string literal
;   @broker.publish.topic.slot    — a publish site's topic OPERAND, literal or not
;   @broker.subscribe.topic.slot  — a subscribe site's topic OPERAND, literal or not
;   @broker.publish.site          — the site a refusal is attributed to and deduped by
;   @broker.subscribe.site        — the site a refusal is attributed to and deduped by
;   @broker.publish.receiver      — the RECEIVER GATE's carrier (see below)
;   @broker.subscribe.receiver    — the RECEIVER GATE's carrier (see below)
;
; ── This file is the STREAMS TOPOLOGY FORM AND NOTHING ELSE ──────────────────
;
; Go had no `brokers` capability before S-408. It gains one here for a single
; shape: the receiver-gated topology link — `Stream(<operand>)` a subscribe,
; `To(<operand>)` a publish ([CR-131] §3.2 A1).
;
; Deliberately NOT ported from the Rust file: its `publish`/`send`/`subscribe`
; verb patterns. Those key on bare method names with no receiver typing, and the
; Rust file's own header records what that costs — on an arbitrary codebase a
; non-broker `.send("literal")` matches, and the claim that it does not is
; measured against Logos's own tree rather than against every project Logos
; indexes. Inheriting a known over-capture in order to look symmetrical would be
; the hand-mirrored-twin failure, not consistency. If a Go story ever wants those
; verbs it should want them receiver-gated, which is now a thing this arm can
; express.
;
; ── WHAT THE ESTATE DOES AND DOES NOT MEASURE HERE ───────────────────────────
;
; Kafka Streams is a JVM library with no first-party Go client, so these patterns
; ship on [CR-131] §10's stakeholder decision — "Streams patterns for Java, Rust
; and Go", taken with the evidence-first concern recorded in the same row. Be
; exact about which half of that has evidence, because the obvious summary
; ("no Go evidence") is wrong in a way that would throw away the better figure:
;
;   CAPTURE — fixture-only. The estate writes no Kafka Streams topology in Go
;   (0 textual `Stream(`/`To(`/`stream(`/`to(` sites), so nothing here exercises
;   whether a Go topology link is captured correctly. That is
;   `extract::broker::go_capture_tests::the_go_topology_form_is_receiver_gated`
;   and nothing else.
;
;   OVER-CAPTURE — MEASURED, on a real corpus. The 84-member reference workspace
;   carries one Go member (`hermodr-mirror`) of **262 `.go` files**, and the
;   receiver-gated arm produces **0 broker rows** over all 262 (measured
;   2026-09-15, asserted in `logos-core/tests/broker_topic_corpus.rs`). That is
;   the false-positive question — the one that actually decides whether a pattern
;   keyed on a verb as common as `To` is shippable — answered on ordinary
;   production Go rather than on a fixture. It is the measurement the Rust
;   `brokers.scm` header records itself as lacking for its own bare-verb
;   patterns, and it is why the gate, not the verb, carries this arm.
;
; So: do not report this arm as "0 captured" without saying which 0. The capture
; half is unexercised; the over-capture half is 0 of 262 and is the evidence.
;
; ── The receiver gate ────────────────────────────────────────────────────────
;
; `Stream` and `To` are ordinary method names, so a bare name predicate here
; would record a broker site for every unrelated `x.To(y)` in a Go tree — the
; manufacture-a-denominator failure S-402 removed from this language's HTTP arm
; by making candidacy receiver-grained (`queries/invocations.scm`, "Candidacy").
; Each pattern therefore captures `@broker.*.receiver`, and
; `extract::broker::receiver_is_topology` admits the match only when that
; receiver's chain bottoms out on a `StreamsBuilder`/`KStream` binding the same
; file declares (`func topology(builder *StreamsBuilder)`, `var s KStream`). Read
; that function's rustdoc before widening either predicate.
;
; BOTH SPELLINGS of each verb are admitted. Go exports by capitalisation, so a
; port of the Streams API exports `Stream`/`To`; the lower-case spelling is what
; an unexported topology helper inside the same package writes. Admitting both
; widens nothing that matters, because the receiver gate — not the verb — is what
; decides the site.
;
; A topic binds iff its operand is a static `(interpreted_string_literal)` or
; `(raw_string_literal)`. Any other operand shape in the enumeration below is
; refused and RECORDED as `topic-not-literal` ([FR-WS-05], [NFR-CC-04]); an
; operand outside the enumeration binds nothing and is not reported, exactly as
; the Java file treats a shape outside its own four.
;
; Droppable on disk at `.logos/plugins/go/queries/brokers.scm` ([FR-PL-04]).

; Subscribe (topology form), BINDING.
(call_expression
  function: (selector_expression
    operand: (_) @broker.subscribe.receiver
    field: (field_identifier) @_st_sub_m)
  arguments: (argument_list
    . [(interpreted_string_literal) (raw_string_literal)] @broker.subscribe.topic)
  (#any-of? @_st_sub_m "Stream" "stream"))

; Subscribe (topology form), REFUSAL slot. The operand shapes are ENUMERATED and
; never a `(_)` wildcard — the Java query's measurement (a wildcard competes with
; the literal pattern at the same argument position and cost it its matches) is
; the reason, and the enumeration makes the question moot here too.
(call_expression
  function: (selector_expression
    operand: (_) @broker.subscribe.receiver
    field: (field_identifier) @_st_sub_slot_m)
  arguments: (argument_list
    . [
      (identifier)
      (selector_expression)
      (call_expression)
      (binary_expression)
    ] @broker.subscribe.topic.slot) @broker.subscribe.site
  (#any-of? @_st_sub_slot_m "Stream" "stream"))

; Publish (topology form), BINDING.
(call_expression
  function: (selector_expression
    operand: (_) @broker.publish.receiver
    field: (field_identifier) @_st_pub_m)
  arguments: (argument_list
    . [(interpreted_string_literal) (raw_string_literal)] @broker.publish.topic)
  (#any-of? @_st_pub_m "To" "to"))

; Publish (topology form), REFUSAL slot — same enumeration, same site grain.
(call_expression
  function: (selector_expression
    operand: (_) @broker.publish.receiver
    field: (field_identifier) @_st_pub_slot_m)
  arguments: (argument_list
    . [
      (identifier)
      (selector_expression)
      (call_expression)
      (binary_expression)
    ] @broker.publish.topic.slot) @broker.publish.site
  (#any-of? @_st_pub_slot_m "To" "to"))
