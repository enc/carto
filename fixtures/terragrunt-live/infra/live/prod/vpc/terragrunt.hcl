include "root" {
  path = find_in_parent_folders("root.hcl")
}

terraform {
  source = "../../../modules//vpc"
}

locals {
  cidr = "10.0.0.0/16"
}

inputs = {
  cidr   = local.cidr
  region = "x"
  unused = 1
}
