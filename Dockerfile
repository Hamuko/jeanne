# BUILD CONTAINER

FROM rust:1.93 AS build

ENV CARGO_NET_GIT_FETCH_WITH_CLI=true

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

FROM gcr.io/distroless/cc-debian13

COPY --from=build /jeanne/target/release/jeanne /bin/jeanne

ENV JEANNE_CONFIG=/config.yaml

ENTRYPOINT ["/bin/jeanne"]
