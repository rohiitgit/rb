# rb-scip-java

A slim, reproducible container image that runs [scip-java](https://github.com/scip-code/scip-java)
0.13.1 against a Maven or Gradle project. Repository Brain uses it to produce `index.scip` for Java
sources; the index is then decoded by `rb-scip` and joined with Tree-sitter facts by `rb-indexer`.

## Why not the official image

`sourcegraph/scip-java` is 6.75 GB: it bundles several JDKs, Gradle, sbt, Bazel and more.
This image is one pinned JDK plus Maven, Gradle and the scip-java launcher, about 1.23 GB unpacked
(about 930 MB before Gradle), and every ingredient is fetched from its canonical URL and verified
against a checksum recorded in the Dockerfile, so a rebuild yields the same tools.

## What is in it

| Component | Source | Verification |
| --- | --- | --- |
| Eclipse Temurin JDK 21 (Ubuntu 26.04) | `eclipse-temurin:21-jdk@sha256:4d06038800655fe1211760cd561de70ef2ed7a47f5d69255e9834414602b7026` (index digest) | pulled by digest |
| Apache Maven 3.9.16 | `https://archive.apache.org/dist/maven/maven-3/3.9.16/binaries/apache-maven-3.9.16-bin.tar.gz` | sha512 `831a8591fe20c8243b1dbe7d71e3244f31d1665b0804b2e825e38cbbe5ce0cafb8338851f90780735568773e0a6cd07bbec107cda0b896b008b861075358b6f6` |
| Gradle 8.14.6 | `https://services.gradle.org/distributions/gradle-8.14.6-bin.zip` | sha256 `7988ed071b2a07900e2ec715fca15c6ab72bce6db433af301fa7fa7e9407bd24` |
| scip-java CLI v0.13.1 | `https://github.com/scip-code/scip-java/releases/download/v0.13.1/scip-java-v0.13.1` (86,321,571 bytes) | sha256 `a694cae143c32c5b6226362fb4bd268a8d13d3cd9b482819b3b0029a9a97b8fe` |

The scip-java release asset is a coursier standalone bootstrap: every jar it needs is embedded,
it only needs `java` on `PATH`, and it makes no network calls. Its javac plugin is embedded too
and is extracted under `/tmp` at index time, so nothing is downloaded during indexing. The base
image already ships `bash`, `curl`, `wget` and `ca-certificates`, so no apt layer is added.

Note: the launcher reports `scip-java version 0.0.0-SNAPSHOT`; the release asset is not stamped
with its version. The checksum above is what identifies it.

Environment set in the image:

- `MAVEN_HOME=/opt/maven`, `/opt/maven/bin` on `PATH`.
- `JAVA_TOOL_OPTIONS="-XX:MaxRAMPercentage=80.0 -XX:+UseContainerSupport"`: applies to every JVM
  started in the container (Maven, the forked javac, the launcher). Each JVM prints a
  `Picked up JAVA_TOOL_OPTIONS` line on stderr; that is normal.
- `JAVA_OPTS` with the five `--add-exports=jdk.compiler/com.sun.tools.javac.*=ALL-UNNAMED`
  flags, mirroring the official image. Only the coursier launcher honours it.
- `WORKDIR /sources`: mount the project there.

## Build

```sh
docker build -t rb-scip-java:0.13.1 docker/scip-java
```

A locally built image has no registry digest; `rb index` resolves the image ID with
`docker image inspect --format '{{.Id}}' rb-scip-java:0.13.1`, runs the containers by that ID and
uses it as the index cache key. When the image is absent, Maven jobs are reported as
`unsupported` and nothing builds the image implicitly.

## Use

Indexing is split into an online fetch phase and an offline index phase, so that the index run
itself can be executed with `--network none`. `$M2` is a host directory that persists the Maven
repository cache between runs (mounted at `/root/.m2`); `$SRC` is the project.

```sh
# 1. Fetch (online): resolve plugins and dependencies, then do one full build.
docker run --rm --network bridge -v "$SRC:/sources" -v "$M2:/root/.m2" rb-scip-java:0.13.1 \
  mvn -B -q org.apache.maven.plugins:maven-dependency-plugin:3.11.0:go-offline
docker run --rm --network bridge -v "$SRC:/sources" -v "$M2:/root/.m2" rb-scip-java:0.13.1 \
  mvn -B -q clean verify -DskipTests

# 2. Index (offline). Arguments after `--` REPLACE scip-java's default Maven arguments
#    (`--batch-mode clean verify -DskipTests`), so all of them are repeated plus `--offline`.
docker run --rm --network none -v "$SRC:/sources" -v "$M2:/root/.m2" rb-scip-java:0.13.1 \
  scip-java index --build-tool Maven -- --batch-mode --offline clean verify -DskipTests
```

`rb index` runs exactly these three steps (`rb_scip::orchestrate::java_runs`) with `$M2` at
`var/scip/cache/m2`, and sets `MAVEN_OPTS=-Dorg.slf4j.simpleLogger.logFile=System.err`: Maven logs
to stdout by default, and only stderr is kept as the tail reported for a failed build.

The result is `/sources/index.scip` (project root `file:///sources/`, document paths relative to
it, e.g. `src/main/java/com/acme/auth/AuthController.java`). scip-java runs Maven as
`mvn -Dmaven.compiler.useIncrementalCompilation=false -Dmaven.compiler.compilerId=javac
-Dmaven.compiler.executable=<extracted javac wrapper> -Dmaven.compiler.fork=true <args>`.

Verified on `fixtures/java-spring-jpa` (Spring Boot 3.3.4 parent, 6 sources): `go-offline` took
about 80 s and left a 281 MB `.m2`; the online `verify` and the offline index each took under
3 s wall-clock and produced a 15.5 KB `index.scip` covering all 6 documents.
