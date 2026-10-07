# rb-egress-proxy

The SCIP fetch phase's egress proxy (contract M5 D15, S21): [tinyproxy](https://tinyproxy.github.io/)
1.11.3 on Alpine 3.24, built locally from the Alpine base already present on the machine of record
(orchestrator decision on fork F2), so no third-party proxy image is pulled.

| Component | Source | Pin |
| --- | --- | --- |
| Alpine 3.24.2 (linux/arm64) | `alpine@sha256:294b683cb724975bec92580e1e685676bd4b50bda910ddb8c51d4cabeaec77e6` | digest |
| tinyproxy (GPL-2.0-or-later; runs as its own container, not linked into `rb`) | Alpine's `main` repository | `tinyproxy=1.11.3-r0` |

The image holds no allow-list. For each fetch run `rb` writes `tinyproxy.conf` and `filter` and
mounts them read-only at `/etc/tinyproxy`: every request is checked by its whole URL against the
filter (`FilterURLs On`, `FilterType fnmatch`, `FilterDefaultDeny Yes`), which lists `host:443`
for each allowed host only, so `CONNECT` to an allowed host on port 443 passes and plain HTTP,
other ports, IP addresses and other hosts are refused with `403`. Every connection and refusal is
logged (`LogLevel Connect`); `rb` reports the refusals of a failed fetch. The proxy runs as the
package's `tinyproxy` user.

## Build

```sh
docker build -t rb-egress-proxy:1.11.3 docker/egress-proxy
```

`apk add` needs the network at build time; the base is used from the local store. Measured on
2026-10-07 (Docker Desktop 29.8.1, linux/arm64): image ID
`sha256:9e2f31c4e334fb7870d4cdbc86f82d86c22dc7a6c0730884d2acdc09dd62c035`, 13,831,923 bytes
(`docker image inspect --format '{{.Size}}'`); a rebuild reuses the cached layer.

`rb index` never builds the image: without it every SCIP job is `unsupported` and the project
stays heuristic, rather than fetching without the proxy.
