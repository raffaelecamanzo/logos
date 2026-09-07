; Java message-broker publish/subscribe capture (S-254, capability = "brokers").
;
; Feeds the generic invocation interpreter (extract::broker) → the
; BrokerPublish / BrokerSubscribe fan-out arm ([FR-WS-10], [ADR-54]). Capture
; vocabulary (interpreted by `extract::broker::capture_broker_invocations`):
;
;   @broker.publish.topic        — a publish site's topic/queue string literal
;   @broker.subscribe.topic      — a subscribe site's topic/queue string literal
;   @broker.subscribe.topic.slot — a subscribe site's topic OPERAND, literal or not
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
; (`PREFIX + "orders"`) — is still refused and binds nothing ([NFR-RA-05]). It is no
; longer *silent*: the `.slot` + `.site` pair records the refusal so it reaches the
; [FR-WS-05] coverage payload as `topic-not-literal`, once per site. A site that
; captured at least one literal records no refusal, so the array and multi-attribute
; forms never report one.
;
; The publish side carries NO `.slot` pattern on purpose. `send`/`convertAndSend`/
; `publish` is a bare method-name predicate, so a slot there would record a refusal
; for every `send(pojo)` in the codebase — manufacturing a coverage denominator out
; of ordinary code. Recognising a real publish site by its topic-bearing header form
; (`MessageBuilder` + `KafkaHeaders.TOPIC`) and recording *its* refusals is
; [CR-117] / S-370's work.
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
