; Rust message-broker publish/subscribe capture (S-291, capability = "brokers").
;
; Feeds the generic invocation interpreter (extract::broker) → the
; BrokerPublish / BrokerSubscribe fan-out arm ([FR-WS-10], [ADR-54]), exactly as
; the Java `brokers.scm` does. Rust already declares `reachability = true`, so
; giving it the `brokers` capability closes the capability-matrix gap
; ([CR-081], [FR-WS-12] AC1): a Rust subscriber handler can now be both dead
; per-repo AND rooted by another member's cross-service publish, so the app-wide
; reachability union view finally has a node to promote on a real index.
;
; Capture vocabulary (interpreted by `capture_broker_invocations`):
;
;   @broker.publish.topic   — a publish site's topic string literal
;   @broker.subscribe.topic — a subscribe site's topic string literal
;
; Only a STATIC `(string_literal)` topic is captured. A dynamically-composed
; topic (a `const` reference, a variable, an `"x".to_owned() + env` expression,
; a `FutureRecord::to(topic_var)` builder) does not match a `(string_literal)`
; node, so it produces no capture and stays honestly unbound — never a guessed
; edge ([NFR-RA-05]). The `@_*` captures exist only for the method-name
; predicates and are ignored by the interpreter.
;
; "Static" is a rule about the operand's grammatical shape, never about the
; characters the literal carries ([CR-107], [FR-WS-10] AC3): `"${env}-orders"` and
; `"braces{only}"` are static string literals and are captured, keyed by their own
; text. A `$`/`{` character rule in `ArtifactRelation::classify_target` used to drop
; them — that rule was language-agnostic, so this file was affected identically to
; the Java one and is repaired by the same change, with no edit here. Asserted by
; `extract::broker::rust_capture_tests::a_rust_topic_literal_binds_whatever_characters_it_carries`.
;
; -- Two audit decisions NOT to change this file ([CR-107] §4.4) --------------
;
; 1. The multi-topic array form was already handled: the rdkafka slice pattern
;    below captures each literal in `subscribe(&["a", "b"])`.
;
; 2. No `@broker.*.topic.slot` / `@broker.*.site` refusal pattern is added here,
;    although the interpreter supports the vocabulary and the Java query uses it.
;    A refusal record is only honest where the site is identifiable as a broker
;    site, and this query keys on bare method verbs with no receiver typing (see
;    the false-positive scope note below). A slot pattern would therefore record a
;    `topic-not-literal` refusal for every `channel.send(x)` and
;    `.subscribe(handler)` in an arbitrary Rust codebase -- manufacturing a
;    coverage denominator out of ordinary code, the failure mode
;    `resolve::framework::drop_non_path_routes` documents on the route side.
;    Recording Rust refusals needs receiver scoping first.
;
; -- S-370 / [CR-117] publish-side audit, recorded EITHER WAY ----------------
;
; S-370 found the Java publish arm blind to the form idiomatic Spring actually
; writes: the topic reaches the message through `setHeader(KafkaHeaders.TOPIC, …)`
; and `send(message)` receives no topic at all. Its criteria require this file to be
; audited for the same publish-side blind spot, and the finding recorded whichever
; way it comes out. It comes out as **no change here**, on three measured grounds.
;
; 1. NO EQUIVALENT FORM EXISTS IN THE RUST CORPUS. The finding the criterion offers
;    as one possible answer is the correct one. Measured 2026-09-07: the 84-member
;    reference workspace contains **0** `.rs` files — it is entirely JVM — and this
;    repository declares no `rdkafka`/broker-client dependency and carries no real
;    producer site of its own (`extract::broker`'s Rust fixtures are the only
;    `.publish("…")`/`.send("…")` strings in the tree, which is the same
;    no-false-positive measurement the scope note below records). There is no Rust
;    broker source to be blind to.
;
; 2. THE HEADER IDIOM HAS NO RUST ANALOGUE. Java's blind spot is specifically a
;    *header constant* — a topic passed as a named header rather than as an
;    argument. Rust broker clients have no header-constant idiom for the topic; the
;    nearest structural analogue is rdkafka's record builder,
;    `FutureRecord::to("orders")`, where the topic sits in a builder method and
;    `producer.send(record, timeout)` carries none. That form is already named as
;    unmatched in this file's header above, and it is a *builder-method* blind spot,
;    not a header one — so it would need its own pattern and its own reasoning, not
;    a port of S-370's.
;
; 3. ADDING THE BUILDER PATTERN SPECULATIVELY WOULD REPEAT THE HAZARD THIS FILE
;    ALREADY RECORDS. A `to("literal")` pattern keys on one of the most common
;    method names in Rust with no receiver typing, which is the same
;    manufacture-a-denominator failure that decision (2) above declines for refusal
;    slots. With no corpus to measure the false-positive rate against, the honest
;    move is to leave the gap named rather than to close it blind.
;
; So: audited, and the finding is that Java's blind spot does not have a Rust twin
; to fix. Recorded as a test — `extract::broker::rust_capture_tests::the_rust_publish_side_has_no_header_form_equivalent_and_is_unchanged`
; — so the gap stays a stated decision rather than an omission, and so a future
; story that acquires a Rust broker corpus finds the reasoning instead of
; re-deriving it.
;
; Vocabulary rationale: Rust has no annotation-based listener idiom (Java's
; `@KafkaListener`), so the capture keys on the generic message-bus method verbs
; a broker client exposes — `publish`/`send` for a producer, `subscribe` for a
; consumer — narrowed to a leading static-string-literal topic. This mirrors the
; Java template-send capture (a method-name predicate + a first-argument string
; literal) and stays honest: a bare `channel.send(struct)` carries no string
; topic and never matches, and the real Logos source tree carries no
; `.send("literal")` / `.publish("literal")` / `.subscribe("literal")` site, so
; the capture adds no spurious broker node when Logos indexes itself.
;
; Scope of that no-false-positive claim: it is measured against Logos's OWN tree,
; not every project Logos may index. On an arbitrary codebase a non-broker
; `.send("literal")` (e.g. a `Sender<&str>` channel) or `.subscribe("literal")`
; could match. The blast radius is bounded on two axes: (1) the broker fan-out and
; the app-wide reachability view it feeds are advisory, never a gate input
; ([ADR-56]) — a spurious topic node cannot move any member's dead-code signal; and
; (2) a lone publish with no cross-member subscriber on the same static topic
; produces no bridge edge and no promotion. A future arm may narrow the producer
; verbs or gate on a receiver-type heuristic if advisory noise is reported.
;
; Droppable on disk at `.logos/plugins/rust/queries/brokers.scm` ([FR-PL-04]).

; ── Subscribe: a consumer method named `subscribe` whose first argument is a
;    topic string literal — bus.subscribe("orders").
(call_expression
  function: (field_expression
    field: (field_identifier) @_sub_m)
  arguments: (arguments
    . (string_literal) @broker.subscribe.topic)
  (#eq? @_sub_m "subscribe"))

; ── Subscribe: the rdkafka slice form — consumer.subscribe(&["orders", "ships"]).
;    Each string literal in the borrowed array is a subscribed topic, attributed
;    to the same enclosing handler.
(call_expression
  function: (field_expression
    field: (field_identifier) @_sub_m2)
  arguments: (arguments
    (reference_expression
      value: (array_expression
        (string_literal) @broker.subscribe.topic)))
  (#eq? @_sub_m2 "subscribe"))

; ── Publish: a producer method named `publish`/`send` whose first argument is a
;    topic string literal — producer.publish("orders", payload),
;    bus.send("orders", payload).
(call_expression
  function: (field_expression
    field: (field_identifier) @_pub_m)
  arguments: (arguments
    . (string_literal) @broker.publish.topic)
  (#any-of? @_pub_m "publish" "send"))
