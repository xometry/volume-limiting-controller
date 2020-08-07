FROM rust:1.45-alpine3.12 as build_env

RUN \
  apk add musl-dev openssl-dev && \
  rustup target add x86_64-unknown-linux-musl

WORKDIR /package-source

COPY Cargo.toml Cargo.lock ./
COPY src src/

RUN cargo build --release --target x86_64-unknown-linux-musl --locked

RUN find -type f

FROM alpine:3.12 AS runtime

RUN apk add openssl

COPY --from=build_env /package-source/target/x86_64-unknown-linux-musl/release/volume-limiting-controller /usr/local/bin/volume-limiting-controller

CMD volume-limiting-controller
