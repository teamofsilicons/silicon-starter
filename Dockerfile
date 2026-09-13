FROM rust:1.98-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p silicon-starter-api
FROM debian:bookworm-slim
RUN useradd --system --uid 10001 starter
COPY --from=build /src/target/release/silicon-starter-api /usr/local/bin/starter-api
USER starter
ENV STARTER_BIND=0.0.0.0:8080
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=5s CMD ["/bin/sh", "-c", "wget -qO- http://127.0.0.1:8080/healthz >/dev/null || exit 1"]
ENTRYPOINT ["/usr/local/bin/starter-api"]
