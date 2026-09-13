FROM rust:1.98-bookworm AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/src/target \
    cargo build --release -p silicon-starter-api && cp target/release/silicon-starter-api /tmp/starter-api
FROM debian:bookworm-slim
RUN useradd --system --uid 10001 starter
COPY --from=build /tmp/starter-api /usr/local/bin/starter-api
USER starter
ENV STARTER_BIND=0.0.0.0:8080
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/starter-api"]
