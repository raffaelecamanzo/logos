; Java message-broker publish/subscribe capture (S-254, capability = "brokers").
;
; Feeds the generic invocation interpreter (extract::broker) → the
; BrokerPublish / BrokerSubscribe fan-out arm ([FR-WS-10], [ADR-54]). Capture
; vocabulary (interpreted by `extract::broker::capture_broker_invocations`):
;
;   @broker.publish.topic   — a publish site's topic/queue string literal
;   @broker.subscribe.topic — a subscribe site's topic/queue string literal
;
; Only a STATIC `(string_literal)` topic is captured. A dynamically-composed
; topic (a constant reference, a variable, a `"x" + env` expression, a
; `topics = {…}` array) does not match a `(string_literal)` node, so it produces
; no capture and stays honestly unbound — never a guessed edge ([NFR-RA-05]).
; The `@_*` captures exist only for the annotation-/method-name predicates.
;
; Droppable on disk at `.logos/plugins/java/queries/brokers.scm`.

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

; ── Subscribe: the single-value annotation form — @KafkaListener("orders").
(method_declaration
  (modifiers
    (annotation
      name: (identifier) @_sub_ann1
      arguments: (annotation_argument_list
        . (string_literal) @broker.subscribe.topic)))
  (#any-of? @_sub_ann1 "KafkaListener" "RabbitListener" "JmsListener"))

; ── Publish: a broker-template send whose first argument is a topic string
;    literal — kafkaTemplate.send("orders", payload),
;    rabbitTemplate.convertAndSend("orders", payload).
(method_invocation
  name: (identifier) @_pub_m
  arguments: (argument_list
    . (string_literal) @broker.publish.topic)
  (#any-of? @_pub_m "send" "convertAndSend" "publish"))
