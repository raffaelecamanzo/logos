; Java message-broker publish/subscribe capture (S-254, capability = "brokers").
;
; Feeds the generic invocation interpreter (extract::broker) → the
; BrokerPublish / BrokerSubscribe fan-out arm ([FR-WS-10], [ADR-54]). Capture
; vocabulary (interpreted by `extract::broker::capture_broker_invocations`):
;
;   @broker.publish.topic        — a publish site's topic/queue string literal
;   @broker.subscribe.topic      — a subscribe site's topic/queue string literal
;   @broker.publish.topic.slot   — a publish site's topic OPERAND, literal or not
;   @broker.subscribe.topic.slot — a subscribe site's topic OPERAND, literal or not
;   @broker.publish.site         — the site a refusal is attributed to and deduped by
;   @broker.subscribe.site       — the site a refusal is attributed to and deduped by
;
; ── What is captured, and what is refused ────────────────────────────────────
;
; A topic binds iff its operand is a STATIC `(string_literal)`. That is the whole
; rule, and it is a rule about the operand's *grammatical shape* — never about the
; characters the literal happens to carry ([FR-WS-10] AC3, [CR-107]). So all of
;
;   "orders"   "dotted.topic.name"   "has-dash"
;   "${spring.kafka.topics.orders}"  "braces{only}"  "dollar$only"
;
; are captured, keyed by the literal's own text. The `${…}` property-placeholder
; form is Spring's *standard* way to write a topic, and it is a static literal like
; any other; a previous `$`/`{` character rule in `ArtifactRelation::classify_target`
; dropped every one of them, which is why a real 84-member Spring estate reported an
; empty topic inventory while 34 files declared Kafka wiring ([CR-107] §2). The topic
; key is the literal as written; resolving a placeholder against committed
; configuration is [CR-117] §3.2's canonical-identity rule, not this file's business.
;
; A `topics = {"a", "b"}` array is one declaration on N topics: each element is
; captured as its own topic, which the `(declaration, topic)` counting contract
; already covers ([FR-WS-11]).
;
; A topic operand that is NOT a string literal — a constant reference
; (`topics = TOPIC`), a field (`Topics.ORDERS`), a concatenation
; (`PREFIX + "orders"`), a call (`config.topic()`) — is still refused and binds
; nothing ([NFR-RA-05]). It is no longer *silent*: the `.slot` + `.site` pair records
; the refusal so it reaches the [FR-WS-05] coverage payload as `topic-not-literal`,
; once per site.
;
; The array and multi-attribute forms never report a refusal, and it is worth being
; exact about why: NOT because the interpreter cancels their candidate, but because
; they never produce one. The slot patterns below enumerate non-literal operand
; shapes, so an `element_value_array_initializer` matches no slot; and
; `#any-of? @_sub_slot_key` excludes a sibling attribute like `containerFactory`.
; The interpreter's own site reconcile is the guard for a DROPPABLE query
; ([FR-PL-04]) that slots an operand another pattern admitted — no query in this
; repository reaches it.
;
; One shape stays deliberately silent: a BLANK literal (`topics = ""` or all
; whitespace) binds nothing and reports nothing. It matches the binding pattern, so
; it produces no refusal candidate, and `broker_topic_key` then refuses its empty
; key. Reporting it would mean slotting `(string_literal)`, which reintroduces the
; overlap hazard described below for the sake of a shape no real listener writes.
;
; The publish side's ARGUMENT form carries no `.slot` pattern, and still does not.
; `send`/`convertAndSend`/`publish` is a bare method-name predicate, so a slot there
; would record a refusal for every `send(pojo)` in the codebase — manufacturing a
; coverage denominator out of ordinary code. The publish arm's refusals come instead
; from its HEADER form (S-370 / [CR-117], the last patterns in this file), where the
; site is identifiable as a broker publish by the `KafkaHeaders.TOPIC` constant it
; names rather than by a method verb every codebase uses. That distinction is the
; whole reason one side of the publish arm reports refusals and the other does not.
;
; The `@_*` captures exist only for the annotation-/method-name predicates and are
; ignored by the interpreter.
;
; Droppable on disk at `.logos/plugins/java/queries/brokers.scm` ([FR-PL-04]).

; ── Subscribe: a Spring listener annotation naming a topic/queue via a
;    key = "value" attribute — @KafkaListener(topics = "orders"),
;    @RabbitListener(queues = "q"), @JmsListener(destination = "d") — on a
;    handler method.
(method_declaration
  (modifiers
    (annotation
      name: (identifier) @_sub_ann
      arguments: (annotation_argument_list
        (element_value_pair
          key: (identifier) @_sub_key
          value: (string_literal) @broker.subscribe.topic))))
  (#any-of? @_sub_ann "KafkaListener" "RabbitListener" "JmsListener")
  (#any-of? @_sub_key "topics" "queues" "destination" "value"))

; ── Subscribe: the multi-topic array attribute form —
;    @KafkaListener(topics = {"orders", "shipments"}). One match per element, so
;    one declaration on N topics yields N subscribe references, all attributed to
;    the same handler ([FR-WS-11]'s (declaration, topic) grain).
(method_declaration
  (modifiers
    (annotation
      name: (identifier) @_sub_arr_ann
      arguments: (annotation_argument_list
        (element_value_pair
          key: (identifier) @_sub_arr_key
          value: (element_value_array_initializer
            (string_literal) @broker.subscribe.topic)))))
  (#any-of? @_sub_arr_ann "KafkaListener" "RabbitListener" "JmsListener")
  (#any-of? @_sub_arr_key "topics" "queues" "destination" "value"))

; ── Subscribe: the single-value annotation form — @KafkaListener("orders").
(method_declaration
  (modifiers
    (annotation
      name: (identifier) @_sub_ann1
      arguments: (annotation_argument_list
        . (string_literal) @broker.subscribe.topic)))
  (#any-of? @_sub_ann1 "KafkaListener" "RabbitListener" "JmsListener"))

; ── Subscribe: the single-value array form — @KafkaListener({"a", "b"}).
(method_declaration
  (modifiers
    (annotation
      name: (identifier) @_sub_arr_ann1
      arguments: (annotation_argument_list
        . (element_value_array_initializer
          (string_literal) @broker.subscribe.topic))))
  (#any-of? @_sub_arr_ann1 "KafkaListener" "RabbitListener" "JmsListener"))

; ── Subscribe REFUSAL slots: a listener whose topic operand is one of the
;    NON-LITERAL shapes below. The interpreter records a `topic-not-literal`
;    refusal for a site that captured no `@broker.subscribe.topic`, so the
;    patterns above decide what binds and these decide only what is *reported*
;    when nothing did ([FR-WS-05], [NFR-CC-04]). The site capture is the dedup
;    grain — one attribute (or one single-value argument list) is one site, so an
;    array of five literals and a five-attribute annotation each refuse at most
;    once.
;
;    The operand shapes are ENUMERATED, never a `(_)` wildcard. A wildcard here
;    is not merely broad, it is wrong: `value: (_)` overlaps the
;    `value: (string_literal)` patterns above on the same node, and tree-sitter
;    then reports only one of the competing patterns per site — measured, it
;    silently cost the *scalar* literal patterns their matches while the array
;    pattern kept its own, so `topics = "${x}"` stopped binding as soon as the
;    refusal slot was added. An enumeration cannot overlap a literal, so the two
;    concerns stay independent.
;
;    The four shapes are exactly [CR-107]'s named cases: a constant reference
;    (`TOPIC`), a qualified field (`Topics.ORDERS`), a concatenation
;    (`PREFIX + "orders"`), and a call (`config.topic()`). An operand of some
;    other shape is simply not reported — the pre-[CR-107] behaviour, never a
;    mis-captured topic.
(method_declaration
  (modifiers
    (annotation
      name: (identifier) @_sub_slot_ann
      arguments: (annotation_argument_list
        (element_value_pair
          key: (identifier) @_sub_slot_key
          value: [
            (identifier)
            (field_access)
            (binary_expression)
            (method_invocation)
          ] @broker.subscribe.topic.slot) @broker.subscribe.site)))
  (#any-of? @_sub_slot_ann "KafkaListener" "RabbitListener" "JmsListener")
  (#any-of? @_sub_slot_key "topics" "queues" "destination" "value"))

(method_declaration
  (modifiers
    (annotation
      name: (identifier) @_sub_slot_ann1
      arguments: (annotation_argument_list
        . [
          (identifier)
          (field_access)
          (binary_expression)
          (method_invocation)
        ] @broker.subscribe.topic.slot) @broker.subscribe.site))
  (#any-of? @_sub_slot_ann1 "KafkaListener" "RabbitListener" "JmsListener"))

; ── Publish: a broker-template send whose first argument is a topic string
;    literal — kafkaTemplate.send("orders", payload),
;    rabbitTemplate.convertAndSend("orders", payload).
(method_invocation
  name: (identifier) @_pub_m
  arguments: (argument_list
    . (string_literal) @broker.publish.topic)
  (#any-of? @_pub_m "send" "convertAndSend" "publish"))

; ── Publish: the HEADER form — the topic reaches the message through a header,
;    not through an argument (S-370, [CR-117]).
;
;    `kafkaTemplate.send("orders", payload)` above is the only publish shape the
;    arm recognised, and idiomatic Spring never writes it. The real form assembles
;    a `Message` and hands the whole thing to `send`:
;
;      Message<SpecificRecord> message = MessageBuilder.withPayload(payload)
;              .setHeader(KafkaHeaders.MESSAGE_KEY, key)
;              .setHeader(KafkaHeaders.TOPIC, topic)
;              .build();
;      kafkaTemplate.send(message);
;
;    The topic is in `setHeader`'s second argument and `send` receives no topic at
;    all. Measured on the 84-member reference estate: **54** sites are written this
;    way, across 52 files (13 of them in `src/main`, spanning 12 members); and the
;    S-339 corpus measurement recorded the complement — **zero** publish topics
;    captured estate-wide, because no publish site anywhere carries a topic-shaped
;    first argument. So [FR-WS-10]'s Statement promise of producer capture yielded 0
;    `Producer` nodes on a 12-member Kafka estate ([CR-117] §2).
;
;    RECOGNISED BY THE HEADER, NEVER BY THE BUILDER. The patterns below name
;    `KafkaHeaders.TOPIC` and say nothing about `MessageBuilder`, for two reasons
;    that pull the same way: a near-miss message builder from another library
;    (`SomeOtherBuilder.setHeader(AmqpHeaders.ROUTING_KEY, …)`) must not
;    over-match, and Spring's topic header is set through several builders
;    (`MessageBuilder`, `MessageHeaderAccessor`, a `Map` of headers) which a
;    builder-typed pattern would have to enumerate and would still miss.
;
;    Three narrowing decisions, each measured against the estate:
;
;    1. **The header is matched QUALIFIED** — `object` = `KafkaHeaders` *and*
;       `field` = `TOPIC`. A `field: (identifier)` predicate on `TOPIC` alone would
;       admit any class's `TOPIC` constant (`MyHeaders.TOPIC`), which is an
;       over-match on a very common constant name. All 54 estate sites write the
;       qualified form; **none** reaches the header through a static import, so the
;       bare `setHeader(TOPIC, …)` form is deliberately NOT recognised. Recognising
;       it would need import resolution, which this layer does not have.
;
;    2. **The method name is a SETTER** — `setHeader`/`setHeaderIfAbsent`. Without
;       it, any two-argument call whose first argument is the topic header matches:
;       `Map.of(KafkaHeaders.TOPIC, t)` is the concrete case, and a header map is
;       not a publish. (A *read* — `headers.get(KafkaHeaders.TOPIC)` — is excluded
;       by arity alone, so it is not what earns this predicate; naming it as the
;       reason would be the plausible-sounding wrong one.) `setHeaderIfAbsent` is
;       `MessageBuilder`'s own sibling setter with identical topic semantics; the
;       estate uses only `setHeader`, so that arm of the predicate is carried on the
;       API's shape rather than on a measurement.
;
;    3. **The two arguments are ANCHORED** by position — the header constant first
;       (the leading `.`), the topic operand immediately after it (the middle `.`).
;       What this does NOT guard against is worth stating, because it is the
;       plausible-sounding wrong reason: a tree-sitter query matches sibling child
;       patterns **in order**, so the operand alternation's `(field_access)` can
;       never be assigned to the header constant itself even though
;       `KafkaHeaders.TOPIC` is a `field_access`. Sibling order handles that. The
;       anchors exclude the case order does not: a call where the topic header is
;       present but is not the first argument, or where the operand is not the one
;       immediately following it — `setHeader(Scope.OUTBOUND, KafkaHeaders.TOPIC,
;       topic)`, which sets a scoped header on some other API and is not a publish
;       site. Covered by `a_near_miss_builder_setting_an_unrelated_header_is_not_a_publish_site`.
;
;    Node shapes established by dumping the parse tree of the estate's real form
;    under the pinned tree-sitter-java 0.23.5 BEFORE these patterns were written
;    (the discipline [CR-107] §4 records, because a pattern tuned until fixtures
;    pass can pass the fixtures and miss the shape):
;
;      (KafkaHeaders.TOPIC, topic)     → (argument_list (field_access object: (identifier) field: (identifier)) (identifier))
;      (KafkaHeaders.TOPIC, "orders")  → (argument_list (field_access …) (string_literal (string_fragment)))
;      (KafkaHeaders.TOPIC, k.get())   → (argument_list (field_access …) (method_invocation object: (identifier) name: (identifier) arguments: (argument_list)))
;      (KafkaHeaders.TOPIC, P + "o")   → (argument_list (field_access …) (binary_expression left: (identifier) right: (string_literal …)))
;
;    WHAT THIS ADMITS ON THE REAL ESTATE: nothing. S-365 measured that **none** of
;    the 54 header-form sites carries a literal topic — 19 pass an identifier (16 of
;    them a method parameter) and 35 read a `@ConfigurationProperties` getter — so
;    recognition alone binds no topic and promotes no `Producer` node. That is the
;    measured outcome, not a defect: what this story delivers is the 54 honest
;    `topic-not-literal` refusals, so a producer-bearing estate stops reading like
;    one with no producer at all ([NFR-CC-04]). Keying a configuration-bound operand
;    against committed configuration is [CR-117] §3.2's canonical-identity rule
;    (S-371), which is unplanned precisely because S-365 falsified its yield on this
;    corpus: 0 of the 13 src/main sites would be admitted even with it.

; Publish (header form), BINDING: the topic operand is a static string literal.
; Keyed by the literal's own text, `${…}` placeholders included — the same
; grammatical-shape rule the subscribe side applies, and the same non-rule about
; the characters it carries.
(method_invocation
  name: (identifier) @_pub_hdr_m
  arguments: (argument_list
    .
    (field_access
      object: (identifier) @_pub_hdr_obj
      field: (identifier) @_pub_hdr_key)
    .
    (string_literal) @broker.publish.topic)
  (#any-of? @_pub_hdr_m "setHeader" "setHeaderIfAbsent")
  (#eq? @_pub_hdr_obj "KafkaHeaders")
  (#eq? @_pub_hdr_key "TOPIC"))

; Publish (header form), REFUSAL slot: the topic operand is one of the NON-LITERAL
; shapes below, so the site is recognised as a publish and reported as
; `topic-not-literal` rather than left silent ([FR-WS-05], [NFR-CC-04]).
;
; The shapes are ENUMERATED, never a `(_)` wildcard — but NOT for the subscribe
; slots' reason, and inheriting their argument here would be wrong. There, a
; `value: (_)` overlaps the `value: (string_literal)` patterns on the same node and
; measurably cost the scalar literal patterns their matches. Measured on this side,
; a wildcard does not do that: the position anchors below and the interpreter's own
; site reconcile between them keep a literal operand binding and cancel the
; candidate a wildcard would raise at that same site.
;
; What the enumeration buys here is PRECISION and one vocabulary across the arm: a
; refusal is reported only for an operand shape the arm has actually reasoned
; about. The four shapes cover every form the estate writes (an identifier, whether
; a method parameter or a `@Value`-injected field; a `@ConfigurationProperties`
; getter call) plus the two it does not but which are idiomatic (a qualified field,
; a concatenation). An operand of some other shape — a ternary, a cast — binds
; nothing (the half that matters, [NFR-RA-05]) and is deliberately not reported,
; exactly as the subscribe side treats a shape outside its own four. That boundary
; is asserted by `an_operand_shape_outside_the_enumeration_binds_nothing_and_is_not_reported`
; so it stays a decision rather than an accident; widening it is a change to BOTH
; sides' enumerations, never a wildcard on one.
;
; The site — and so the dedup grain, one refusal per site — is the `setHeader`
; call's own `argument_list`. Unlike the publish arm's bare `send`/`publish`
; method-name predicate, which is why no slot was ever added there, this site is
; identifiable as a broker publish by the header constant it names: the refusal is
; recorded for a real publish, not manufactured out of every `send(pojo)` in the
; codebase.
(method_invocation
  name: (identifier) @_pub_hdr_slot_m
  arguments: (argument_list
    .
    (field_access
      object: (identifier) @_pub_hdr_slot_obj
      field: (identifier) @_pub_hdr_slot_key)
    .
    [
      (identifier)
      (field_access)
      (binary_expression)
      (method_invocation)
    ] @broker.publish.topic.slot) @broker.publish.site
  (#any-of? @_pub_hdr_slot_m "setHeader" "setHeaderIfAbsent")
  (#eq? @_pub_hdr_slot_obj "KafkaHeaders")
  (#eq? @_pub_hdr_slot_key "TOPIC"))
