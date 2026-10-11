# rb-scip-go

A slim, reproducible container image that runs [scip-go](https://github.com/scip-code/scip-go)
v0.2.7 against a Go module. Repository Brain uses it to produce `index.scip` for Go
sources; the index is then decoded by `rb-scip` and joined with Tree-sitter facts by `rb-indexer`.

## Why not the official image

`sourcegraph/scip-go` is stale (300.7 MB compressed, last updated long before the
`scip-code` fork's v0.2.x line), and it bundles a full Go release plus build tooling with no
pinned digests. This image is Ubuntu 24.04 plus a pinned Go toolchain plus a scip-go built
from a pinned source commit, and every ingredient is fetched from its canonical URL and
verified against a checksum recorded in the Dockerfile, so a rebuild yields the same tools.

## What is in it

| Component | Source | Verification |
| --- | --- | --- |
| Ubuntu 24.04 + `ca-certificates`, `curl`, `git` | `ubuntu:24.04@sha256:08571ca13e00ca07a2a84eab83a959b4242e22cceb16486a11bef1428c9e93a7` (arm64 index digest) | pulled by digest |
| Go 1.27.2 (`linux-$TARGETARCH`) | `https://go.dev/dl/go1.27.2.linux-$TARGETARCH.tar.gz` | sha256 `ecbadb99…96045fe5` (amd64), `94f3e30b…0214c6b7c8` (arm64) |
| scip-go CLI v0.2.7 | tag `v0.2.7` of `github.com/scip-code/scip-go`, commit `2e9ff3c2603a85daabe125c9f20075ec52df0731` | tag fetched, commit verified equal; `go.sum`-verified build |

The scip-go module cache is dropped after the build, so no third-party module sources linger
in the image. `GOTOOLCHAIN=local` keeps the toolchain from self-updating at run time;
`GOFLAGS=-mod=mod` lets the fetch phase resolve the module graph. `WORKDIR /sources`:
mount the project there.

Environment set in the image:

- `PATH=/usr/local/go/bin:/usr/local/bin:...`, so `go` and `scip-go` are on `PATH`.
- `GOTOOLCHAIN=local`, `GOFLAGS=-mod=mod` (see above).

## Build

```sh
docker build -t rb-scip-go:0.2.7 docker/scip-go
```

A locally built image has no registry digest; `rb index` resolves the image ID with
`docker image inspect --format '{{.Id}}' rb-scip-go:0.2.7`, runs the containers by that ID and
uses it as the index cache key. When the image is absent, Go jobs are reported as
`unsupported` and nothing builds the image implicitly.
