resource "aws_lambda_function" "handler" {
  function_name = "h-${var.region}"
}
