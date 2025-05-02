# BUILD CONTAINER

FROM rust:1.86 AS build

ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

RUN apt-get update && apt-get install -y ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN USER=root cargo new --bin jeanne

# Build dependencies separately for layer caching.
WORKDIR /jeanne
COPY ./Cargo.lock ./Cargo.lock
COPY ./Cargo.toml ./Cargo.toml
RUN cargo build --release

# Clean the temporary project.
RUN rm src/*.rs ./target/release/deps/jeanne*

ADD . ./
RUN cargo build --release --verbose


# RUNTIME CONTAINER

FROM debian:bookworm-slim

COPY --from=build /etc/ssl/certs/ /etc/ssl/certs/

COPY --from=build /jeanne/target/release/jeanne .

ENV JEANNE_CONFIG=/config.yaml

CMD ["./jeanne"]
