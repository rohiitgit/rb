// In-process and empty-URL request checks (contract M6b S4): these requests
// never leave the process (or have no URL), so they must not stitch to
// endpoints. Mirrors mattermost's handler tests, which build requests for
// in-process handlers.
package main

import (
	"net/http"
	"net/http/httptest"
	"testing"
)

func echoCompat(w http.ResponseWriter, _ *http.Request) {
	w.WriteHeader(http.StatusAccepted)
}

// A request served in-process from a recorder: no CONSUMES edge, whatever
// its URL (here a foreign absolute URL that would otherwise stitch).
func TestServeMfaInProcess(t *testing.T) {
	handler := http.HandlerFunc(echoCompat)
	rec := httptest.NewRecorder()
	req, err := http.NewRequest(http.MethodPost, "http://notification:3000/internal/mfa/email", nil)
	if err != nil {
		t.Fatal(err)
	}
	handler.ServeHTTP(rec, req)
	if rec.Code != http.StatusAccepted {
		t.Fatalf("code = %d", rec.Code)
	}
}

// A request handed to a handler alongside its recorder: the recorder value
// marks the call in-process, with no ServeHTTP in sight.
func TestProxyNotifyInProcess(t *testing.T) {
	rec := httptest.NewRecorder()
	req, err := http.NewRequest(http.MethodPost, "http://notification:3000/internal/mfa/email", nil)
	if err != nil {
		t.Fatal(err)
	}
	deliverEcho(rec, req)
}

func deliverEcho(w http.ResponseWriter, r *http.Request) {
	echoCompat(w, r)
}

// An empty URL is never path-matched: no edge and its own bucket, even
// next to a plain handler call with no recorder in it.
func TestEmptyRequestURL(t *testing.T) {
	req, err := http.NewRequest(http.MethodGet, "", nil)
	if err != nil {
		t.Fatal(err)
	}
	if !checkToken(req) {
		t.Fatal("empty request rejected")
	}
}

func checkToken(r *http.Request) bool {
	return r.URL.Scheme == ""
}
