resource "aws_cloudwatch_metric_alarm" "orders_lag" {
  alarm_name  = "orders-processing-lag-dev"
  metric_name = "Orders.ProcessingLag"
}
