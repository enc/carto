include "root" {
  path = find_in_parent_folders("root.hcl")
}

terraform {
  source = "${get_terragrunt_dir()}/../../../modules/app"
}

dependency "vpc" {
  config_path = "../vpc"
}

dependency "ghost" {
  config_path = "../ghost"
}

dependency "abs" {
  config_path = "${get_repo_root()}/infra/live/prod/vpc"
}

dependencies {
  paths = ["../vpc"]
}

locals {
  missing_out = dependency.vpc.outputs.does_not_exist
}

inputs = {
  vpc_id = dependency.vpc.outputs.vpc_id
  region = "x"
  nope   = 1
}
