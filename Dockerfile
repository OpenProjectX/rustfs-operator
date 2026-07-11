FROM rust:1.97.0-trixie AS builder
WORKDIR /build
COPY Cargo.toml ./
COPY src ./src
RUN cargo build --release --bin rustfs-operator

FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 65532 nonroot
COPY --from=builder /build/target/release/rustfs-operator /usr/local/bin/rustfs-operator
USER 65532
ENTRYPOINT ["/usr/local/bin/rustfs-operator"]
CMD ["run"]
