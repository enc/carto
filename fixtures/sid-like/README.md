# sid-like fixture

Synthetic, minimal reproduction of the cross-language string-literal
contract shape described in the SID Cloud repo spec (ADR-0026/0027) —
not real customer code. Exercises the acceptance test that motivated
the feature: *"which Terraform CloudWatch metric names are never
emitted by any service?"*

| Path | Exercises |
|---|---|
| `services/ingest/EmfMetricsExportService.cs`'s `Namespace` const | qualifier derivation — a same-file `const string Namespace = "...";` |
| `services/ingest/EmfMetricsExportService.cs`'s `quoteDrops` | `object-init:Name` producer capture, matched by `infra/alarms.tf`'s `quote_drops` alarm |
| `services/ingest/EmfMetricsExportService.cs`'s `kafkaErrors` | produced, never consumed — an orphan on the *producer* side |
| `services/ingest/EmfMetricsExportService.cs`'s `dynamicMetric` | a computed (non-literal) `Name` value — structurally invisible, not extracted at all (INV-8 honesty; not counted as an "uncaptured site" this slice — see ADR-0026's scope note) |
| `infra/alarms.tf`'s `quote_drops` | `aws_cloudwatch_metric_alarm.metric_name` consumer capture, `namespace` sibling attribute as qualifier |
| `infra/alarms.tf`'s `sequence_gaps` | **the acceptance-test shape**: consumed, never produced — a dead alarm that can never fire |
| `infra/alarms.tf`'s `dynamic_alarm` | an interpolated `metric_name` (`"${local.prefix}Drops"`) — structurally distinguishable from a plain literal at parse time, silently not extracted; must never appear in `orphans` output either direction |
| `infra/alarms.tf`'s `streamer_errors` | same bare metric name (`KafkaErrorsTotal`) as the C# file's orphaned emission, but under a *different* namespace — must not cross-match; guards the qualifier-discipline failure mode the SID spec's `CUSTOMER_SUBSCRIPTIONS_TABLE`/`DYNAMO_CUSTOMER_SUBSCRIPTIONS_TABLE` example warns about |
| `infra/locals.tf.simu` | the two-part `.tf.<env>` extension walk fix — classified `Lang::Hcl` even though `Path::extension()` alone would see `simu` |

Verify with:

```bash
cargo run -p carto-cli -- index fixtures/sid-like --out /tmp/carto-sid
cargo run -p carto-cli -- orphans fixtures/sid-like --out /tmp/carto-sid --category metric_name
```

`orphans --category metric_name` must return exactly:
- `consumed_never_produced`: `SequenceGapsTotal` (namespace `SIDCloud/Ingest`), `KafkaErrorsTotal` (namespace `SIDCloud/Streamer`)
- `produced_never_consumed`: `KafkaErrorsTotal` (namespace `SIDCloud/Ingest`)

`QuoteDropsPerSecond` and the interpolated alarm must appear in neither list.
