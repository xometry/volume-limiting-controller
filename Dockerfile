FROM rust:1.45 as build_env

RUN apt-get update -y && apt-get install -y libssl-dev

WORKDIR /package-source

COPY Cargo.toml Cargo.lock ./
COPY src src/

RUN cargo build --release --locked

RUN find -type f

FROM ubuntu:20.04 AS runtime

RUN apt-get update -y && apt-get install -y openssl

COPY --from=build_env /package-source/target/release/volume-limiting-controller /usr/local/bin/volume-limiting-controller

CMD volume-limiting-controller
