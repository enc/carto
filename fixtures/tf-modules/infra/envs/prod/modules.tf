module "vpc" {
  source = "../../modules/vpc"
  cidr   = "10.0.0.0/16"
  region = var.region
  bogus  = 1
}

module "app" {
  source = "../../modules/app"
  region = var.region
}

module "registry_vpc" {
  source  = "terraform-aws-modules/vpc/aws"
  version = "5.0.0"
}

module "from_git" {
  source = "git::https://ci-user:tok3n-S3CR3T@example.com/org/net.git//modules/net?ref=v1.2.0&sshkey=abc"
}

module "missing_dir" {
  source = "../../modules/nope"
}

output "vpc_id" {
  value = module.vpc.vpc_id
}

output "bad_output" {
  value = module.vpc.nonexistent
}
