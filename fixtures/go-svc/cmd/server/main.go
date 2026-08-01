package main

import (
	"fmt"

	"github.com/acme/svc/internal/orders"
	_ "github.com/lib/pq"
)

func main() {
	order := orders.ParseOrder("42")
	fmt.Println(order)
	unknownExternalCall()
}
