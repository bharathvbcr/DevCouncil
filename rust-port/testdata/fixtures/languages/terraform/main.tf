variable "name" { type = string }
output "message" { value = "hello ${var.name}" }
