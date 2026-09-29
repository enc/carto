resource "aws_lambda_function" "handler" {
  function_name = "x-${var.vpc_id}"
}
