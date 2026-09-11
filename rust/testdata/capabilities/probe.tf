module "helper" {
  source = "./helper"
}

resource "aws_s3_bucket" "b" {
  bucket = lower(var.name)
}

output "name" {
  value = format("%s", aws_s3_bucket.b.bucket)
}
