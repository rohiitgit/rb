package invoice

import "database/sql"

// Total sums line amounts in cents.
func Total(lines []int64) int64 {
	var t int64
	for _, l := range lines {
		t += l
	}
	return t
}

// MarkPaid flags an invoice as paid.
func MarkPaid(db *sql.DB, id int64) error {
	_, err := db.Exec(`UPDATE billing.invoices SET paid = true WHERE id = $1`, id)
	return err
}

// Purge empties a table chosen at runtime; its name is not knowable statically.
func Purge(db *sql.DB, table string) error {
	_, err := db.Exec("DELETE FROM " + table)
	return err
}

// Owner returns the e-mail of an invoice's owner from the accounts table auth maintains.
func Owner(db *sql.DB, id int64) (string, error) {
	var email string
	err := db.QueryRow("SELECT email FROM users WHERE id = $1", id).Scan(&email)
	return email, err
}
