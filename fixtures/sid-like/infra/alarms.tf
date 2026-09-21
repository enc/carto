resource "aws_cloudwatch_metric_alarm" "quote_drops" {
  alarm_name          = "quote-drops-alarm"
  namespace           = "SIDCloud/Ingest"
  metric_name         = "QuoteDropsPerSecond"
  treat_missing_data  = "notBreaching"
}

# The acceptance-test shape: a metric name no service emits. Always sits
# in OK (treat_missing_data = "notBreaching") and can never fire.
resource "aws_cloudwatch_metric_alarm" "sequence_gaps" {
  alarm_name          = "sequence-gaps-alarm"
  namespace           = "SIDCloud/Ingest"
  metric_name         = "SequenceGapsTotal"
  treat_missing_data  = "notBreaching"
}

# Interpolated metric_name — structurally distinguishable from a plain
# literal at parse time and silently not extracted (ADR-0026). Must
# never appear as an orphan in either direction.
resource "aws_cloudwatch_metric_alarm" "dynamic_alarm" {
  alarm_name          = "dynamic-alarm"
  namespace           = "SIDCloud/Ingest"
  metric_name         = "${local.prefix}Drops"
  treat_missing_data  = "notBreaching"
}

# Same bare metric name as EmfMetricsExportService.cs's KafkaErrorsTotal
# emission, but under a *different* namespace — must not cross-match.
# Since nothing in this namespace emits it, it's its own orphan.
resource "aws_cloudwatch_metric_alarm" "streamer_errors" {
  alarm_name          = "streamer-errors-alarm"
  namespace           = "SIDCloud/Streamer"
  metric_name         = "KafkaErrorsTotal"
  treat_missing_data  = "notBreaching"
}
