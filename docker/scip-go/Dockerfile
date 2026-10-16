# Slim, reproducible scip-go image (see README.md).
#
# Why it differs from rb-scip-go:0.2.7: Go 1.27.2 writes unified export data version 5
# (go.dev/issue/81188, backported to 1.27.2). scip-go v0.2.7 pins golang.org/x/tools v0.45.0,
# whose decoder reads at most version 4, so every import (the standard library included)
# loads as an empty package and its values are `invalid type`. x/tools v0.50.0 is the first
# release that reads version 5. This image builds scip-go v0.2.7's pinned commit with only
# x/tools raised to v0.50.0 (and what MVS raises with it). The resulting go.mod and go.sum are
# pinned by sha256, and a self-test fails the build if scip-go cannot type an import.
#
# Base: Ubuntu 24.04, pinned by digest. The Go toolchain tarball is checksum-verified.
# No apt layer beyond ca-certificates, curl and git.
FROM ubuntu:24.04@sha256:08571ca13e00ca07a2a84eab83a959b4242e22cceb16486a11bef1428c9e93a7

ARG GO_VERSION=1.27.2
ARG GO_SHA256_AMD64=ecbadb99091a3f46e31f5f934b068b1864eafa7995211b39eaddf76996045fe5
ARG GO_SHA256_ARM64=94f3e30b8e374bc285e7dadc11e0865726b9bc6e85b841ccceaabc0214c6b7c8
ARG SCIP_GO_VERSION=0.2.7
ARG SCIP_GO_COMMIT=2e9ff3c2603a85daabe125c9f20075ec52df0731
# The one dependency raised over v0.2.7's go.mod (first x/tools whose pkgbits reads V5).
ARG XTOOLS_VERSION=v0.50.0
# sha256 of scip-go's go.mod and go.sum after `go get golang.org/x/tools@${XTOOLS_VERSION}`.
# The module graph is fixed by these two files; go.sum's h1 hashes were checked against
# sum.golang.org when they were first written (GOSUMDB on, its default, during the build).
ARG SCIP_GO_GOMOD_SHA256=ed3b5d9e15a45903c98b04952d7a75b0bb5d3158341163dd22991759ab665d7e
ARG SCIP_GO_GOSUM_SHA256=8ce9cab71d9165da797e8387b1af2c68ef3b948bdf3f9a9be68315db356da51e
ARG TARGETARCH

RUN set -eux; \
    apt-get update && apt-get install -y --no-install-recommends ca-certificates curl git; \
    rm -rf /var/lib/apt/lists

# Go toolchain, checksum-verified against the value published on go.dev.
RUN set -eux; \
    case "${TARGETARCH}" in \
        amd64) GO_SHA256="${GO_SHA256_AMD64}" ;; \
        arm64) GO_SHA256="${GO_SHA256_ARM64}" ;; \
        *) echo "unsupported architecture ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    curl -fsSL -o /tmp/go.tar.gz \
      "https://go.dev/dl/go${GO_VERSION}.linux-${TARGETARCH}.tar.gz"; \
    echo "${GO_SHA256}  /tmp/go.tar.gz" | sha256sum -c -; \
    tar -xzf /tmp/go.tar.gz -C /usr/local; \
    rm -f /tmp/go.tar.gz

# scip-go v0.2.7 from its tag, commit verified, x/tools raised, go.mod/go.sum verified by
# sha256, then built. `go version -m` must report the raised x/tools. The module cache and
# sources are dropped after the build so no third-party sources linger in the image.
RUN set -eux; \
    export PATH=/usr/local/go/bin:${PATH} GOCACHE=/tmp/gocache GOMODCACHE=/tmp/gomod \
           GOTOOLCHAIN=local GOFLAGS=-mod=mod; \
    git clone --depth 1 --branch "v${SCIP_GO_VERSION}" https://github.com/scip-code/scip-go.git /tmp/scip-go; \
    test "$(git -C /tmp/scip-go rev-parse HEAD)" = "${SCIP_GO_COMMIT}"; \
    cd /tmp/scip-go; \
    go get "golang.org/x/tools@${XTOOLS_VERSION}"; \
    go mod verify; \
    sha256sum go.mod go.sum; \
    git --no-pager diff --stat; \
    echo "${SCIP_GO_GOMOD_SHA256}  go.mod" | sha256sum -c -; \
    echo "${SCIP_GO_GOSUM_SHA256}  go.sum" | sha256sum -c -; \
    GOFLAGS=-mod=readonly go build -trimpath -o /usr/local/bin/scip-go ./cmd/scip-go; \
    go version -m /usr/local/bin/scip-go | grep -E "^\s+dep\s+golang.org/x/tools\s+${XTOOLS_VERSION}\s"; \
    rm -rf /tmp/scip-go /tmp/gocache /tmp/gomod

# Self-test, offline: scip-go must type a standard-library call. With an x/tools that cannot
# read the toolchain's export data, `os` loads empty and no occurrence of
# `scip-go gomod github.com/golang/go/src <go version> os/Getenv().` is written (the control:
# rb-scip-go:0.2.7 writes none; var/m9c/logs/p0b/selftest-control.txt).
RUN set -eux; \
    export PATH=/usr/local/go/bin:${PATH} GOCACHE=/tmp/gocache GOTOOLCHAIN=local GOFLAGS=-mod=mod; \
    mkdir -p /tmp/selftest && cd /tmp/selftest; \
    printf 'module selftest\n\ngo 1.25\n' > go.mod; \
    printf 'package main\n\nimport "os"\n\nfunc main() { _ = os.Getenv("HOME") }\n' > main.go; \
    scip-go; \
    grep -aq ' os/Getenv()[.]' index.scip; \
    cd / && rm -rf /tmp/selftest /tmp/gocache

ENV PATH=/usr/local/go/bin:/usr/local/bin:${PATH} \
    GOTOOLCHAIN=local \
    GOFLAGS=-mod=mod

WORKDIR /sources
