FROM --platform=linux/amd64 694713800774.dkr.ecr.us-east-2.amazonaws.com/golden/rust:1.86.0-dev AS build-dev

USER root
RUN apk add --no-cache build-base perl linux-headers curl ca-certificates
ARG OPENSSL_VERSION=1.1.1w
RUN curl -sSL https://www.openssl.org/source/openssl-${OPENSSL_VERSION}.tar.gz \
  | tar -xz -C /tmp \
  && cd /tmp/openssl-${OPENSSL_VERSION} \
  && ./config no-shared --prefix=/usr/local/openssl-1.1 --openssldir=/usr/local/openssl-1.1 \
  && make -j"$(nproc)" \
  && make install_sw \
  && rm -rf /tmp/openssl-${OPENSSL_VERSION}
USER nonroot
ENV OPENSSL_DIR=/usr/local/openssl-1.1 OPENSSL_LIB_DIR=/usr/local/openssl-1.1/lib OPENSSL_INCLUDE_DIR=/usr/local/openssl-1.1/include OPENSSL_STATIC=1

WORKDIR /package-source

COPY Cargo.toml Cargo.lock ./
COPY src src/

USER root
RUN cargo update -p socket2 --precise 0.3.19 && chown -R nonroot:nonroot /package-source
USER nonroot
RUN cargo build --release --locked

FROM --platform=linux/amd64 694713800774.dkr.ecr.us-east-2.amazonaws.com/golden/glibc-dynamic:15.1.0 AS runtime

USER 65532:65532

COPY --from=build-dev /package-source/target/release/volume-limiting-controller /usr/local/bin/volume-limiting-controller
COPY --from=build-dev /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt

ENTRYPOINT ["/usr/local/bin/volume-limiting-controller"]
