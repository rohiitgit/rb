// Command billing serves the invoicing API.
package main

import (
	"log"
	"net/http"

	"github.com/gin-gonic/gin"
)

func main() {
	r := gin.Default()
	api := r.Group("/billing")
	api.POST("/invoices/:id/pay", payInvoice)
	// Legacy MFA echo kept for old clients; notification owns the real endpoint.
	r.POST("/internal/mfa/email", mfaEcho)
	log.Fatal(r.Run(":8080"))
}

func payInvoice(c *gin.Context) {
	if err := notifyPaid(); err != nil {
		c.Status(http.StatusBadGateway)
		return
	}
	c.Status(http.StatusNoContent)
}

func mfaEcho(c *gin.Context) {
	c.Status(http.StatusAccepted)
}

func notifyPaid() error {
	resp, err := http.Post("http://notification:3000/internal/mfa/email", "application/json", nil)
	if err != nil {
		return err
	}
	return resp.Body.Close()
}
