// Synthetic constants for the redaction fixture (spec §7.5, §11.1,
// ADR-0017). Every "secret" below is fake/placeholder-shaped — not
// real customer code, not a real credential.
package main

const dbConnection = "postgres://admin:hunter2example@db.internal:5432/orders"
const slackWebhookToken = "xoxb-1234567890-abcdefghijklmnop"
const gitlabToken = "glpat-1234567890abcdefWXYZ"
const jwtExample = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dQw4w9WgXcQ-rDpZ5RwFj0"

// Clean: an ordinary low-entropy constant.
const serviceName = "orders-api"

func Configure() {}
