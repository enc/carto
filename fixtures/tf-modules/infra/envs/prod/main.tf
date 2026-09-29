locals {
  bucket_name = "${local.prefix}-${var.name_suffix}"
  broken_name = "${local.missing}-x"
}

resource "aws_s3_bucket" "logs" {
  bucket = local.bucket_name
  region = var.region
}

resource "aws_s3_bucket_policy" "logs" {
  bucket = aws_s3_bucket.logs.id

  dynamic "statement" {
    for_each = var.name_suffix == "" ? [] : [1]
    content {
      sid = statement.value
    }
  }
}

output "bucket_arn" {
  value = aws_s3_bucket.logs.arn
}
