package main

import "database/sql"

func lookupInvoice(db *sql.DB, id string) (string, error) {
	var total string
	err := db.QueryRow("SELECT total FROM invoices WHERE id = ?", id).Scan(&total)
	return total, err
}

func recordPayment(db *sql.DB, id string) {
	_, _ = db.Exec("INSERT INTO payments (id) VALUES (?)", id)
}

func clearCache() {}
