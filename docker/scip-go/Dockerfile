# Slim, reproducible scip-go image (see README.md).
#
# Base: Ubuntu 24.04, pinned by digest. A Go toolchain tarball and a scip-go
# built from a pinned source commit are added, both checksum-verified, so a
# rebuild yields the same tools. No apt layer beyond ca-certificates and git:
# the Go toolchain needs roots to speak HTTPS to the module proxy, and `go`
# needs git for direct VCS fetches (both behind the egress proxy at run time).
FROM ubuntu:24.04@sha256:08571ca13e00ca07a2a84eab83a959b4242e22cceb16486a11bef1428c9e93a7

ARG GO_VERSION=1.27.2
ARG GO_SHA256_AMD64=ecbadb99091a3f46e31f5f934b068b1864eafa7995211b39eaddf76996045fe5
ARG GO_SHA256_ARM64=94f3e30b8e374bc285e7dadc11e0865726b9bc6e85b841ccceaabc0214c6b7c8
ARG SCIP_GO_VERSION=0.2.7
ARG SCIP_GO_COMMIT=2e9ff3c2603a85daabe125c9f20075ec52df0731
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

# scip-go v0.2.7: built from source (no usable published image or standalone
# release asset exists). The tag is fetched and its commit verified, so a moved
# tag cannot smuggle in other sources. The module cache is dropped after the
# build so no third-party sources linger in the image.
RUN set -eux; \
    export PATH=/usr/local/go/bin:${PATH} GOCACHE=/tmp/gocache GOMODCACHE=/tmp/gomod; \
    git clone --depth 1 --branch "v${SCIP_GO_VERSION}" https://github.com/scip-code/scip-go.git /tmp/scip-go; \
    test "$(git -C /tmp/scip-go rev-parse HEAD)" = "${SCIP_GO_COMMIT}"; \
    cd /tmp/scip-go && go build -trimpath -o /usr/local/bin/scip-go ./cmd/scip-go; \
    rm -rf /tmp/scip-go /tmp/gocache /tmp/gomod

ENV PATH=/usr/local/go/bin:/usr/local/bin:${PATH} \
    GOTOOLCHAIN=local \
    GOFLAGS=-mod=mod

WORKDIR /sources
