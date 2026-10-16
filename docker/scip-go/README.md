# rb-scip-go

A slim, reproducible container image that runs [scip-go](https://github.com/scip-code/scip-go)
v0.2.7, built with golang.org/x/tools v0.50.0, against a Go module. Repository Brain uses it to
produce `index.scip` for Go sources; the index is then decoded by `rb-scip` and joined with
Tree-sitter facts by `rb-indexer`.

## Why not the official image

`sourcegraph/scip-go` is stale (300.7 MB compressed, last updated long before the
`scip-code` fork's v0.2.x line), and it bundles a full Go release plus build tooling with no
pinned digests. This image is Ubuntu 24.04 plus a pinned Go toolchain plus a scip-go built
from a pinned source commit, and every ingredient is fetched from its canonical URL and
verified against a checksum recorded in the Dockerfile, so a rebuild yields the same tools.

## Why x/tools v0.50.0

scip-go loads imported packages from the export data the Go toolchain writes. Go 1.27.2 writes
unified export data version 5 (golang/go#81188, backported in 1.27.2). scip-go v0.2.7 pins
x/tools v0.45.0, whose decoder reads at most version 4, so every import, the standard library
included, loaded as an empty package: values of imported types were `invalid type`, and
dependency members got no occurrence. scip-go reports none of this and exits 0. v0.50.0 is the
first x/tools release that reads version 5. No scip-go release after v0.2.7 exists, so the image
builds v0.2.7's commit with only x/tools raised (contract M9c amendment 2). The previous image,
`rb-scip-go:0.2.7`, is that degraded build.

## What is in it

| Component | Source | Verification |
| --- | --- | --- |
| Ubuntu 24.04 + `ca-certificates`, `curl`, `git` | `ubuntu:24.04@sha256:08571ca13e00ca07a2a84eab83a959b4242e22cceb16486a11bef1428c9e93a7` (arm64 index digest) | pulled by digest |
| Go 1.27.2 (`linux-$TARGETARCH`) | `https://go.dev/dl/go1.27.2.linux-$TARGETARCH.tar.gz` | sha256 `ecbadb99…96045fe5` (amd64), `94f3e30b…0214c6b7c8` (arm64) |
| scip-go CLI v0.2.7 | tag `v0.2.7` of `github.com/scip-code/scip-go`, commit `2e9ff3c2603a85daabe125c9f20075ec52df0731` | tag fetched, commit verified equal |
| golang.org/x/tools v0.50.0 | `go get golang.org/x/tools@v0.50.0` in scip-go's module (MVS also raises x/mod v0.36.0 → v0.41.0, x/sync v0.20.0 → v0.23.0, x/sys v0.44.0 → v0.48.0, and the `go` line 1.25.0 → 1.26.0) | `go.sum` hashes checked against sum.golang.org when written; `go mod verify`; the resulting `go.mod` and `go.sum` pinned by sha256 (`ed3b5d9e…ab665d7e`, `8ce9cab7…db356da51e`), so any drift fails the build; built with `-mod=readonly`; `go version -m` must list `golang.org/x/tools v0.50.0` |

The scip-go sources and module cache are dropped after the build, so no third-party module
sources linger in the image. `GOTOOLCHAIN=local` keeps the toolchain from self-updating at run
time; `GOFLAGS=-mod=mod` lets the fetch phase resolve the module graph. `WORKDIR /sources`:
mount the project there.

Environment set in the image:

- `PATH=/usr/local/go/bin:/usr/local/bin:...`, so `go` and `scip-go` are on `PATH`.
- `GOTOOLCHAIN=local`, `GOFLAGS=-mod=mod` (see above).

## Self-test

The build ends with an offline self-test: scip-go indexes a module whose `main` calls
`os.Getenv`, and the build fails unless the index holds the symbol
`scip-go gomod github.com/golang/go/src <go version> os/Getenv().`. An x/tools that cannot read
the toolchain's export data writes no such symbol (`rb-scip-go:0.2.7` fails the same test), so
the next export data version bump breaks the build, not the indexes.

## Build

```sh
docker build -t rb-scip-go:0.2.7-xtools0.50.0 docker/scip-go
```

A locally built image has no registry digest; `rb index` resolves the image ID with
`docker image inspect --format '{{.Id}}' rb-scip-go:0.2.7-xtools0.50.0`, runs the containers by
that ID and uses it as the index cache key. The ID changes with every build, even from cached
layers (the attestation manifest differs), so a rebuild re-indexes every Go job once. When the
image is absent, Go jobs are reported as `unsupported` and nothing builds the image implicitly.
