package main

import (
	"database/sql"
	"net/http"

	"github.com/go-chi/chi/v5"
)

// Payer is satisfied implicitly: Card and Cash declare no implements clause.
type Payer interface {
	Pay(amount int) string
}

// Card pays through the shared charge helper.
type Card struct {
	Number string
}

func (c Card) Pay(amount int) string { return charge(c.Number, amount) }

// Cash pays without any helper.
type Cash struct{}

func (Cash) Pay(amount int) string { return "cash" }

// Checkout dispatches through the interface: scip-go relates both
// implementations to this call.
func Checkout(p Payer, amount int) string { return p.Pay(amount) }

func charge(number string, amount int) string { return number }

var ledger *sql.DB

func main() {
	r := chi.NewRouter()
	r.Get("/health", health)
	r.Route("/billing", func(r chi.Router) {
		r.Post("/pay", payInvoice)
		r.With(receipt).Get("/receipt", showReceipt)
	})
	r.Group(func(r chi.Router) {
		r.Delete("/cache", purgeCache)
	})
	r.Handle("/raw", rawHandler)
	_ = Checkout(Card{Number: "4111"}, 100)
	_ = Checkout(Cash{}, 5)
	_ = http.ListenAndServe(":8080", r)
}

func health(w http.ResponseWriter, req *http.Request) {
	_, _ = lookupInvoice(ledger, "h-1")
}

func payInvoice(w http.ResponseWriter, req *http.Request) {
	_ = Checkout(Card{Number: "4111"}, 100)
	recordPayment(ledger, "p-1")
}

func showReceipt(w http.ResponseWriter, req *http.Request) {
	_, _ = lookupInvoice(ledger, "r-1")
}

func receipt(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, req *http.Request) {
		next.ServeHTTP(w, req)
	})
}

func purgeCache(w http.ResponseWriter, req *http.Request) {
	clearCache()
}

func rawHandler(w http.ResponseWriter, req *http.Request) {}
