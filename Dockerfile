FROM rust:1.45 as build_env

RUN \
  rustup show; \
  cargo version; \
  rustc --version;

WORKDIR /package-source

COPY Cargo.toml Cargo.lock ./
COPY src src/

RUN cargo build --target x86_64-unknown-linux-musl --locked

FROM alpine:3.10 AS runtime

COPY --from build_env /package-source/target/x86_64-unknown-linux-musl/volume-limiting-controller /usr/local/bin/volume-limiting-controller

CMD volume-limiting-controller
