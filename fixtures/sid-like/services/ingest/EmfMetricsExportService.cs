namespace SIDCloud.Ingest;

public class EmfMetricsExportService
{
    private const string Namespace = "SIDCloud/Ingest";

    public void Emit()
    {
        // Matched by infra/alarms.tf's "quote_drops" alarm.
        var quoteDrops = new { Name = "QuoteDropsPerSecond", Unit = "Count/Second" };

        // Emitted, but no alarm anywhere references it — orphan on the
        // "produced, never consumed" side.
        var kafkaErrors = new { Name = "KafkaErrorsTotal", Unit = "Count" };

        // Computed at runtime — not a plain string literal, so this is
        // structurally invisible to the extractor (INV-8's honesty
        // extended to RawLiteral, ADR-0026). Deliberately left
        // uncounted this slice; see the ADR's "slice 1" scope note.
        var dynamicMetric = new { Name = ComputeDynamicMetricName(), Unit = "Count" };
    }

    private string ComputeDynamicMetricName()
    {
        return "Computed" + "MetricName";
    }
}
