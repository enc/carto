resource "aws_cloudwatch_metric_alarm" "orders_lag" {
  alarm_name  = "orders-processing-lag-prod"
  metric_name = "Orders.ProcessingLag"
}
