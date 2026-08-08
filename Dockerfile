FROM ghcr.io/openprojectx/dockerhub/library/rust:1.97.0-trixie AS base

# Normalize the manifest so routine version bumps don't invalidate the
# dependency layer below.
FROM base AS manifest
COPY Cargo.toml Cargo.lock /normalized/
RUN sed -i 's/^version = ".*"/version = "0.0.0"/' /normalized/Cargo.toml \
    && sed -i '/^name = "rustfs-operator"$/{n;s/^version = ".*"/version = "0.0.0"/}' /normalized/Cargo.lock

FROM base AS builder
WORKDIR /build

# Dependency layer: build all dependencies against dummy sources so the
# layer is keyed on the (normalized) Cargo.toml only and survives source
# changes. Persisted across CI runs by the buildx GHA cache, seeded from
# master builds (tag runs can only restore default-branch caches).
COPY --from=manifest /normalized/Cargo.toml /normalized/Cargo.lock ./
RUN mkdir src \
    && echo 'fn main() {}' > src/main.rs \
    && touch src/lib.rs \
    && cargo build --release --locked --bin rustfs-operator \
    && rm -rf src

COPY Cargo.toml Cargo.lock ./
COPY src ./src
# touch so cargo rebuilds the crate itself against the real sources
RUN touch src/main.rs src/lib.rs \
    && cargo build --release --locked --bin rustfs-operator

FROM ghcr.io/openprojectx/dockerhub/library/debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 65532 nonroot
COPY --from=builder /build/target/release/rustfs-operator /usr/local/bin/rustfs-operator
USER 65532
ENTRYPOINT ["/usr/local/bin/rustfs-operator"]
CMD ["run"]
