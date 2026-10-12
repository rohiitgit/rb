# Slim, reproducible scip-java image (see README.md).
#
# Base: Eclipse Temurin JDK 21 on Ubuntu 26.04, pinned by index digest. It already ships
# bash, curl, wget and ca-certificates, so no apt layer is needed.
FROM eclipse-temurin:21-jdk@sha256:4d06038800655fe1211760cd561de70ef2ed7a47f5d69255e9834414602b7026

ARG MAVEN_VERSION=3.9.16
ARG MAVEN_SHA512=831a8591fe20c8243b1dbe7d71e3244f31d1665b0804b2e825e38cbbe5ce0cafb8338851f90780735568773e0a6cd07bbec107cda0b896b008b861075358b6f6
ARG SCIP_JAVA_VERSION=0.13.1
ARG SCIP_JAVA_SHA256=a694cae143c32c5b6226362fb4bd268a8d13d3cd9b482819b3b0029a9a97b8fe
ARG GRADLE_VERSION=8.14.6
ARG GRADLE_SHA256=7988ed071b2a07900e2ec715fca15c6ab72bce6db433af301fa7fa7e9407bd24

# Apache Maven, checksum-verified against the value published on archive.apache.org.
RUN set -eux; \
    curl -fsSL -o /tmp/maven.tar.gz \
      "https://archive.apache.org/dist/maven/maven-3/${MAVEN_VERSION}/binaries/apache-maven-${MAVEN_VERSION}-bin.tar.gz"; \
    echo "${MAVEN_SHA512}  /tmp/maven.tar.gz" | sha512sum -c -; \
    mkdir -p /opt/maven; \
    tar -xzf /tmp/maven.tar.gz -C /opt/maven --strip-components=1; \
    rm -f /tmp/maven.tar.gz

# Gradle, checksum-verified against the value published on services.gradle.org.
# The repository's wrapper is never used (contract M6e D2); the image's
# Gradle is the only distribution the fetch and index phases may run.
# (`unzip` is not in the image; the JDK's `jar` extracts the distribution.)
RUN set -eux; \
    curl -fsSL -o /tmp/gradle.zip \
      "https://services.gradle.org/distributions/gradle-${GRADLE_VERSION}-bin.zip"; \
    echo "${GRADLE_SHA256}  /tmp/gradle.zip" | sha256sum -c -; \
    cd /opt && jar xf /tmp/gradle.zip; \
    mv "/opt/gradle-${GRADLE_VERSION}" /opt/gradle; \
    chmod 0755 /opt/gradle/bin/gradle; \
    rm -f /tmp/gradle.zip

# scip-java CLI: a coursier standalone bootstrap (all jars embedded; needs only `java` on PATH).
# The javac plugin is embedded in it and extracted at index time; scip-java makes no network calls.
RUN set -eux; \
    curl -fsSL -o /usr/local/bin/scip-java \
      "https://github.com/scip-code/scip-java/releases/download/v${SCIP_JAVA_VERSION}/scip-java-v${SCIP_JAVA_VERSION}"; \
    echo "${SCIP_JAVA_SHA256}  /usr/local/bin/scip-java" | sha256sum -c -; \
    chmod 0755 /usr/local/bin/scip-java

ENV MAVEN_HOME=/opt/maven \
    GRADLE_HOME=/opt/gradle \
    PATH=/opt/gradle/bin:/opt/maven/bin:${PATH} \
    # Applies to every JVM started in the container (Maven, javac, the launcher).
    JAVA_TOOL_OPTIONS="-XX:MaxRAMPercentage=80.0 -XX:+UseContainerSupport" \
    # Honoured by the coursier launcher only; mirrors the official scip-java image.
    JAVA_OPTS="--add-exports=jdk.compiler/com.sun.tools.javac.api=ALL-UNNAMED --add-exports=jdk.compiler/com.sun.tools.javac.code=ALL-UNNAMED --add-exports=jdk.compiler/com.sun.tools.javac.model=ALL-UNNAMED --add-exports=jdk.compiler/com.sun.tools.javac.tree=ALL-UNNAMED --add-exports=jdk.compiler/com.sun.tools.javac.util=ALL-UNNAMED"

WORKDIR /sources
