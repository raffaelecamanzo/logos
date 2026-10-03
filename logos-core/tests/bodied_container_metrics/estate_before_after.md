# S-502 / CR-163 — pec-services before/after: metric semantics v6 → v7

Measured 2026-10-03. **These figures are findings, not acceptance floors**: no
test reads them, and a later re-measurement that differs is a new finding, not
a regression. This table is estate-gated: it is invisible to `gate.sh` and to
CI, which have no estate.

## How it was measured

- **Estate:** `~/source/pec-services`, 84 independent git clones. The estate was
  only read: each member was exported with `git -C <member> archive HEAD` and
  extracted twice into a scratch directory, so both binaries scored identical
  trees and neither ran in the estate or touched its enrolled stores.
- **Before:** PATH `logos` 1.8.3 (metric semantics v6), sha256 `5393809c16bd92f1…`.
- **After:** this branch's release build (`cargo build --release -p logos` at
  `e3dbf1d8`, after the S-502 review fixes; it also reports `1.8.3`, the
  version is not bumped until release), sha256 `df7d1fdb581e4384…` (metric
  semantics v7: S-501's duplicate floor, bodied LCOM4 with bodyless hooks kept
  as connectors, bodied Focus). The first measurement, with the pre-review
  build (`dc9ade35`, sha256 `82a4f5a78bae51c6…`), produced identical figures
  for every member: no class on the estate reaches the template-method case
  the review fix changed.
- Each copy: `git init`, `logos index`, `logos --json scan`; the duplicate count
  and bodyless share read the copy's `.logos/logos.db` read-only. Members ran
  one at a time. Harness: `measure_estate.py`; table: `render_table.py`; raw
  per-member rows: `estate_results.jsonl` (all beside this file).
- **Denominator: 84 of 84 members.** 70 score a signal under both binaries,
  11 have no production code (empty-graph `n/a` sentinel under both: they hold
  only YAML, Markdown, HTTP files or scripts), and 3 are empty clones with no
  commit (`ERROR` rows: nothing to archive). 54 members carry `src/main/java`.

## Columns

Values read `old → new`; a single value means unchanged. **Signal** is the
0–10000 aggregate; **Redundancy**, **Cohesion**, **Focus** are the normalized
dimensions; **`is_duplicate`** counts production functions flagged as exact
duplicates; **Bodyless share** is the new store's production callables
recorded `has_body = 0` (the old store has no such column); the last column is
the first Cohesion worst offender (`—` when there is none).

## What moved

Computed by `render_table.py --summary estate_results.jsonl`, never typed by
hand:

- Members: 84; scored under both binaries: 70; empty-graph n/a: 11; error rows: 3; with `src/main/java`: 54.
- Signal: rose on 53, unchanged on 17, fell on 0; median change +234, mean +366, largest +1759 (`timestamp-service-client`).
- Redundancy: rose on 53, fell on 0, unchanged on 17.
- Cohesion: rose on 17, fell on 0, unchanged on 53.
- Focus: rose on 4, fell on 0, unchanged on 66.
- Uniqueness: rose on 0, fell on 0, unchanged on 70.
- Production `is_duplicate`: 6283 → 2136.
- God containers: 32 → 28.
- Bodyless production callables: 844 of 11368 (7.4%).
- Top Cohesion offender changed on 13 members.

Reading the rows: the Cohesion and Focus moves are the declarative mappers the
CR is about — `mailbox-manager`'s `MailboxMapper` (LCOM4 23, a god container
at 23 methods over 106 lines) gives way to `MongoMailboxOperationService`
(LCOM4 9) and is no longer god; `archive-api`'s `ArchiveMapper` gives way to
`ArchiveControllerV1`; `pecserver-facade`'s `PecServerMapper` (27 methods) is
no longer god. No dimension changed applicability on any member.

The largest signal moves come from Redundancy on members whose duplicates were
small generated or constant-returning methods (`timestamp-service-client`,
`legacy-official-log-tools`, `*-kafka-models`, `styleguide`), not from
Cohesion or Focus: their top Cohesion offender (e.g. a JAXB `ObjectFactory`,
LCOM4 24/62, all methods bodied) is unchanged.

## Per member

| Member | Java | Signal | Redundancy | Cohesion | Focus | `is_duplicate` | Bodyless share | Top Cohesion offender (old → new) |
|---|---|---|---|---|---|---|---|---|
| mailbox-manager | yes | 6933 → 7455 | 0.396 → 0.798 | 0.637 → 0.646 | 0.988 → 1.000 | 203 → 68 | 17.0% (57/336) | MailboxMapper (LCOM4 23) → MongoMailboxOperationService (LCOM4 9) |
| agid-volume-counters-batch | yes | 7853 → 8039 | 0.791 → 1.000 | 0.581 | 1.000 | 9 → 0 | 0.0% (0/43) | Counters (LCOM4 7) |
| archive-api | yes | 8356 → 8518 | 0.838 → 1.000 | 0.808 → 0.821 | 1.000 | 6 → 0 | 21.6% (8/37) | ArchiveMapper (LCOM4 6) → ArchiveControllerV1 (LCOM4 3) |
| archive-api-logiclens-fork | yes | 8337 → 8499 | 0.838 → 1.000 | 0.808 → 0.821 | 1.000 | 6 → 0 | 21.6% (8/37) | ArchiveMapper (LCOM4 6) → ArchiveControllerV1 (LCOM4 3) |
| archive-feeder | yes | 8072 | 1.000 | 0.833 | 1.000 | 0 | 0.0% (0/20) | ArchiveEventProducer (LCOM4 3) |
| archive-kafka-models | yes | 7031 → 7871 | 0.333 → 1.000 | 0.688 → 0.708 | 1.000 | 8 → 0 | 25.0% (3/12) | KafkaKeyMapper (LCOM4 4) → KafkaKeyMapper (LCOM4 3) |
| archive-listener-adapter | yes | 8112 → 8336 | 0.696 → 0.913 | 0.685 | 1.000 | 7 → 2 | 21.7% (5/23) | KafkaProducer (LCOM4 3) |
| archive-manager | yes | 7742 → 8092 | 0.639 → 0.983 | 0.638 → 0.645 | 1.000 | 43 → 2 | 18.5% (22/119) | AggregateMapper (LCOM4 8) → KafkaMessagingService (LCOM4 6) |
| archive-notification-adapter | yes | 8932 | 1.000 | 0.708 | 1.000 | 0 | 0.0% (0/11) | MessageTransformer (LCOM4 3) |
| archive-reporting-adapter | yes | 8761 | 1.000 | 0.708 | 1.000 | 0 | 0.0% (0/11) | MessageTransformer (LCOM4 3) |
| batch-test | yes | n/a | 1.000 | n/a | n/a | 0 | n/a | — |
| filters-api | yes | 7801 → 8284 | 0.548 → 1.000 | 0.719 | 1.000 | 14 → 0 | 29.0% (9/31) | RequestLoggingFilter (LCOM4 4) |
| filters-domain | yes | 7588 → 8349 | 0.385 → 1.000 | 0.500 | 1.000 | 8 → 0 | 46.2% (6/13) | AbstractMongoConfiguration (LCOM4 2) |
| filters-kafka-models | yes | 7432 → 8262 | 0.429 → 1.000 | 0.375 → 0.417 | 1.000 | 4 → 0 | 14.3% (1/7) | KafkaKeyMapper (LCOM4 4) → KafkaKeyMapper (LCOM4 3) |
| filters-processor | yes | 8484 | 1.000 | 0.708 | 1.000 | 0 | 0.0% (0/13) | MessageTransformer (LCOM4 3) |
| funnel-aggregator-api | yes | 7484 → 7932 | 0.508 → 0.908 | 0.766 | 1.000 | 64 → 12 | 33.1% (43/130) | FunnelAggregatorControllerV1 (LCOM4 9) |
| legacy-official-log-tools | yes | 6339 → 7542 | 0.173 → 0.985 | 0.385 | 0.921 | 974 → 18 | 8.8% (104/1178) | ObjectFactory (LCOM4 62) |
| mailbox-aggregator-api | yes | 7453 → 7886 | 0.537 → 0.935 | 0.648 → 0.651 | 0.981 → 0.987 | 157 → 22 | 31.3% (106/339) | MailboxMapper (LCOM4 26) → MailboxAggregatorService (LCOM4 26) |
| mailbox-api | yes | 7772 → 8154 | 0.607 → 0.981 | 0.759 | 0.988 | 81 → 4 | 25.2% (52/206) | MailboxControllerV1 (LCOM4 17) |
| mailbox-domain | yes | 7299 → 8091 | 0.357 → 1.000 | 0.444 | 0.950 | 63 → 0 | 45.9% (45/98) | MailboxRepositoryImpl (LCOM4 18) |
| mailbox-kafka-models | yes | 6948 → 7778 | 0.333 → 1.000 | 0.550 → 0.567 | 1.000 | 10 → 0 | 20.0% (3/15) | KafkaKeyMapper (LCOM4 4) → KafkaKeyMapper (LCOM4 3) |
| mailbox-notification-adapter | yes | 8770 | 1.000 | 0.708 | 1.000 | 0 | 0.0% (0/11) | MessageTransformer (LCOM4 3) |
| mailbox-reporting-adapter | yes | 8467 | 1.000 | 0.667 | 1.000 | 0 | 6.2% (1/16) | MessageTransformer (LCOM4 3) |
| mailbox-unread-mail-batch | yes | 8691 → 8838 | 0.846 → 1.000 | 0.750 | 1.000 | 2 → 0 | 7.7% (1/13) | JobConfiguration (LCOM4 4) |
| mailserver-api | yes | 7432 → 8121 | 0.412 → 1.000 | 0.444 | 1.000 | 10 → 0 | 0.0% (0/17) | MailServerApiConfiguration (LCOM4 6) |
| notification-adapter | yes | 7919 → 8102 | 0.761 → 0.957 | 0.889 | 0.957 | 11 → 2 | 6.5% (3/46) | NotificationConsumer (LCOM4 3) |
| notification-api | yes | 7822 → 8309 | 0.571 → 1.000 | 0.662 → 0.692 | 1.000 | 18 → 0 | 21.4% (9/42) | EmailAddressNotificationPreferencesMapper (LCOM4 5) → RequestLoggingFilter (LCOM4 4) |
| notification-domain | yes | 7391 → 8353 | 0.294 → 1.000 | 0.850 | 1.000 | 12 → 0 | 17.6% (3/17) | MongoConfiguration (LCOM4 4) |
| notification-kafka-models | yes | 9725 | 1.000 | n/a | 1.000 | 0 | 0.0% (0/1) | — |
| official-log-batch-lib | yes | 8154 → 8284 | 0.822 → 0.918 | 0.635 → 0.666 | 1.000 | 13 → 6 | 27.4% (20/73) | AmazonS3Client (LCOM4 7) |
| official-log-daily-activities-fetcher-batch | yes | 7993 → 8295 | 0.789 → 1.000 | 0.583 → 0.667 | 1.000 | 4 → 0 | 10.5% (2/19) | DayFetchStepConfiguration (LCOM4 4) |
| official-log-export-api | yes | 8300 → 8522 | 0.767 → 1.000 | 0.801 | 1.000 | 10 → 0 | 14.0% (6/43) | RequestLoggingFilter (LCOM4 4) |
| official-log-export-domain | yes | 8225 → 8465 | 0.750 → 1.000 | 0.500 | 1.000 | 2 → 0 | 50.0% (4/8) | MongoOfficialLogConfiguration (LCOM4 2) |
| official-log-export-job-manager | yes | 7796 → 8264 | 0.536 → 0.952 | 0.679 → 0.684 | 1.000 | 39 → 4 | 17.9% (15/84) | ExportJobMapper (LCOM4 9) → KafkaMessagingService (LCOM4 7) |
| official-log-export-job-worker | yes | 8163 → 8251 | 0.867 → 0.962 | 0.695 → 0.697 | 1.000 | 14 → 4 | 15.2% (16/105) | OfficialLogMapper (LCOM4 5) → ZIPService (LCOM4 5) |
| official-log-export-notification-adapter | yes | 8932 | 1.000 | 0.708 | 1.000 | 0 | 0.0% (0/11) | MessageTransformer (LCOM4 3) |
| official-log-ingestion-batch | yes | 8012 → 8184 | 0.776 → 0.959 | 0.728 | 1.000 | 11 → 2 | 12.2% (6/49) | TSDCreationStepConfiguration (LCOM4 5) |
| official-log-ingestion-domain | yes | 6437 → 7341 | 0.269 → 1.000 | 0.158 | 1.000 | 68 → 0 | 50.5% (47/93) | PdvLegalStorageJournalRepositoryImpl (LCOM4 17) |
| official-log-ingestion-status-batch | yes | 7737 → 8029 | 0.649 → 0.935 | 0.618 → 0.622 | 1.000 | 27 → 5 | 9.1% (7/77) | AcceptedPdvCheckStepConfiguration (LCOM4 5) |
| official-log-journal-api | yes | 7136 → 7817 | 0.343 → 0.852 | 0.558 | 1.000 | 71 → 16 | 26.9% (29/108) | PdvLegalStorageJournalMetricConfiguration (LCOM4 6) |
| official-log-kafka-models | yes | 8158 → 9755 | 0.200 → 1.000 | 1.000 | 1.000 | 4 → 0 | 40.0% (2/5) | — |
| official-log-legal-storage-batch | yes | 7990 → 8087 | 0.886 → 1.000 | 0.742 | 1.000 | 8 → 0 | 10.0% (7/70) | LegalStoragePushStepConfiguration (LCOM4 6) |
| pecserver-facade | yes | 7691 → 8194 | 0.492 → 0.899 | 0.695 → 0.710 | 0.982 → 0.991 | 126 → 25 | 24.2% (60/248) | PecServerMapper (LCOM4 14) → PecServerMapper (LCOM4 10) |
| postel-cpx-creation-lib | yes | 7659 → 7869 | 0.857 → 1.000 | 0.510 → 0.573 | 1.000 | 6 → 0 | 2.4% (1/42) | CpxIndexBuilder (LCOM4 4) |
| punctuators-poc | yes | 7828 → 8102 | 0.708 → 1.000 | 0.625 | 1.000 | 7 → 0 | 0.0% (0/24) | VolumeCountersDeserializer (LCOM4 3) |
| reporting-api | yes | 7836 → 8064 | 0.750 → 1.000 | 0.666 | 1.000 | 17 → 0 | 19.1% (13/68) | ReportingControllerV1 (LCOM4 8) |
| reporting-archive-data-downsampler | yes | 8398 | 1.000 | 0.633 | 1.000 | 0 | 0.0% (0/14) | ForwardingPunctuator (LCOM4 5) |
| reporting-archive-data-projector | yes | 8568 | 1.000 | 0.778 | 1.000 | 0 | 0.0% (0/7) | ArchiveVolumeCountersConsumer (LCOM4 3) |
| reporting-data-export-batch | yes | 7310 → 7683 | 0.574 → 0.944 | 0.504 | 1.000 | 23 → 3 | 16.7% (9/54) | MailboxesExtractionStepConfiguration (LCOM4 5) |
| reporting-domain | yes | 7179 → 8246 | 0.250 → 1.000 | 1.000 | 1.000 | 21 → 0 | 57.1% (16/28) | — |
| reporting-kafka-models | yes | 7836 → 8990 | 0.333 → 1.000 | n/a | 1.000 | 2 → 0 | 0.0% (0/3) | — |
| reporting-mailbox-data-projector | yes | 8563 | 1.000 | 0.778 | 1.000 | 0 | 0.0% (0/6) | MailboxReportingConsumer (LCOM4 3) |
| reporting-volume-counters-batch | yes | 7915 → 8099 | 0.795 → 1.000 | 0.581 | 1.000 | 9 → 0 | 0.0% (0/44) | Counters (LCOM4 7) |
| virus-ingestion-batch | yes | 8669 | 1.000 | 0.750 | 1.000 | 0 | 5.6% (1/18) | JobConfiguration (LCOM4 2) |
| batch-kafka-models | no | n/a | 1.000 | n/a | n/a | 0 | n/a | — |
| deprecated-mailbox-core | no | 7329 → 7837 | 0.441 → 0.852 | 0.768 → 0.769 | 0.982 → 0.991 | 174 → 46 | 22.5% (70/311) | MailboxMapper (LCOM4 20) → KafkaMessagingService (LCOM4 13) |
| e2e-tests | no | n/a | 1.000 | n/a | n/a | 0 | n/a | — |
| filters-ingestion-cdk | no | 8788 → 8882 | 0.833 → 0.917 | n/a | 1.000 | 8 → 4 | 0.0% (0/48) | — |
| gen2gen3_devops_utils | no | 8392 → 8421 | 0.899 → 0.924 | n/a | n/a | 8 → 6 | 0.0% (0/79) | — |
| hermodr-gateway | no | n/a | 1.000 | n/a | n/a | 0 | n/a | — |
| hermodr-mirror | no | 6949 → 7196 | 0.678 → 0.929 | n/a | 1.000 | 232 → 51 | 0.0% (0/721) | — |
| mailbox-aggregate-hermodr-gateway | no | n/a | 1.000 | n/a | n/a | 0 | n/a | — |
| mailserver-common | no | 7717 → 7995 | 0.702 → 1.000 | 0.747 | 1.000 | 25 → 0 | 13.1% (11/84) | QuotaUsageUpdatedListener (LCOM4 4) |
| mock-legal-storage | no | n/a | 1.000 | n/a | n/a | 0 | n/a | — |
| notification-gateway-mock | no | n/a | 1.000 | n/a | n/a | 0 | n/a | — |
| official-log-export-reporting-adapter | no | ERROR: no HEAD commit (empty clone) — nothing to measure | | | | | | |
| official-log-metrics-adapter | no | ERROR: no HEAD commit (empty clone) — nothing to measure | | | | | | |
| pec-agid-statistics-informer | no | 7923 → 8177 | 0.729 → 1.000 | 0.806 | 1.000 | 55 → 0 | 0.0% (0/203) | StatisticsController (LCOM4 4) |
| pec-services-tools | no | 8694 → 9160 | 0.625 → 1.000 | 0.708 | 1.000 | 3 → 0 | 0.0% (0/8) | MongodbConfiguration (LCOM4 3) |
| pecserver-mock | no | n/a | 1.000 | n/a | n/a | 0 | n/a | — |
| pecserver-reporting-adapter | no | ERROR: no HEAD commit (empty clone) — nothing to measure | | | | | | |
| poste-pec-common | no | 9109 | 1.000 | 0.833 | 1.000 | 0 | 25.0% (6/24) | TraceableKafkaComponent (LCOM4 3) |
| poste-pec-documentation | no | 8732 → 8778 | 0.872 → 0.915 | n/a | 1.000 | 6 → 4 | 0.0% (0/47) | — |
| poste-pec-starter | no | n/a | 1.000 | n/a | n/a | 0 | n/a | — |
| ptpec-assistance-tool | no | 8745 | 0.727 | n/a | n/a | 3 | 0.0% (0/11) | — |
| ptpec-precommit-hooks | no | n/a | 1.000 | n/a | n/a | 0 | n/a | — |
| retail-migration-pre-population-script | no | 8136 | 1.000 | n/a | 1.000 | 0 | 0.0% (0/16) | — |
| software-architecture-documents | no | 7984 → 8283 | 0.692 → 1.000 | 0.823 | 1.000 | 4 → 0 | 0.0% (0/13) | BillingConfiguration (LCOM4 4) |
| spring-cloud-data-flow-helm | no | 9570 | 1.000 | n/a | n/a | 0 | 0.0% (0/4) | — |
| spring-cloud-data-flow-pecserver-task | no | 7815 | 1.000 | n/a | n/a | 0 | 0.0% (0/14) | — |
| sprint-review-tools | no | n/a | 1.000 | n/a | n/a | 0 | n/a | — |
| styleguide | no | 5784 → 7055 | 0.101 → 0.495 | n/a | n/a | 3158 → 1773 | 0.0% (0/3513) | — |
| timestamp-service-client | no | 5520 → 7279 | 0.063 → 1.000 | 0.377 | 0.967 | 164 → 0 | 0.0% (0/175) | ObjectFactory (LCOM4 24) |
| webmail | no | 7737 → 7822 | 0.872 → 0.972 | 0.608 | 0.841 | 251 → 54 | 0.4% (7/1954) | Plugin (LCOM4 27) |
