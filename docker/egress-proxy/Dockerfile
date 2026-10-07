# The fetch-phase egress proxy (contract M5 D15, S21): tinyproxy on the pinned Alpine base that is
# already on the machine of record, built locally rather than pulled (orchestrator decision on F2).
#
# Base: alpine 3.24.2 (linux/arm64), pinned by digest. Package: tinyproxy 1.11.3-r0 from Alpine's
# own repository, pinned by version (`apk add` needs the network at build time only).
# rb generates the proxy's configuration per fetch run (the allow-list) and mounts it read-only at
# /etc/tinyproxy/tinyproxy.conf; the image holds no allow-list of its own.
FROM alpine@sha256:294b683cb724975bec92580e1e685676bd4b50bda910ddb8c51d4cabeaec77e6

RUN apk add --no-cache tinyproxy=1.11.3-r0

USER tinyproxy
EXPOSE 8888
ENTRYPOINT ["tinyproxy", "-d", "-c", "/etc/tinyproxy/tinyproxy.conf"]
