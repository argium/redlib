# Release packaging image: unpacks the musl binary that the "Release Build" workflow
# built and checksummed into dist/. To build from source, use Dockerfile.alpine.
ARG ALPINE_VERSION=3.23

FROM alpine:${ALPINE_VERSION} AS unpack

ARG TARGET=x86_64-unknown-linux-musl

COPY dist/redlib-${TARGET}.tar.gz dist/redlib-${TARGET}.tar.gz.sha256 /dist/
RUN cd /dist \
    && sha256sum -c "redlib-${TARGET}.tar.gz.sha256" \
    && tar xzf "redlib-${TARGET}.tar.gz" -C /usr/local/bin/

FROM alpine:${ALPINE_VERSION} AS release

COPY --from=unpack /usr/local/bin/redlib /usr/local/bin/redlib

# Add non-root user for running redlib
RUN adduser --home /nonexistent --no-create-home --disabled-password redlib
USER redlib

# Tell Docker to expose port 8080
EXPOSE 8080

# Healthy once /healthz answers 200, which needs a live OAuth token, so allow time to start.
HEALTHCHECK --interval=30s --timeout=10s --start-period=30s --retries=3 CMD ["redlib", "--healthcheck"]

CMD ["redlib"]
