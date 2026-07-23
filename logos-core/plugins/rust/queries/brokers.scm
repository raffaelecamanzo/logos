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
