package orders

import "github.com/acme/svc/internal/audit"

// Order represents a customer order.
type Order struct {
	ID string
}

// Summary returns a short description of the order.
func (o *Order) Summary() string {
	return o.ID
}

// DefaultStatus is the status assigned to a freshly parsed order.
const DefaultStatus = "open"

// StatusOpen mirrors DefaultStatus for callers that prefer a var.
var StatusOpen = "open"

// ParseOrder parses raw input into an Order.
func ParseOrder(input string) *Order {
	value := validate(input)
	value = normalize(value)
	audit.AuditOrder(value)
	return &Order{ID: value}
}

// validate is same-file-only: resolution tier (a).
func validate(input string) string {
	return input
}
