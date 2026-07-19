FROM rust:1.96.0-bookworm@sha256:5e2214abe154fe26e39f64488952e5c991eeed1d6d6da7cc8381ae83927f0cfc AS builder
WORKDIR /src

COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM debian:bookworm-slim@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libcap2-bin \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/target/release/safexip /usr/bin/safexip
COPY LICENSE-APACHE LICENSE-MIT /usr/share/licenses/safexip/
RUN groupadd --gid 10001 safexip \
    && useradd --uid 10001 --gid 10001 --no-create-home --shell /usr/sbin/nologin safexip \
    && setcap cap_net_bind_service=+ep /usr/bin/safexip

EXPOSE 53/udp 53/tcp 8080/tcp
LABEL org.opencontainers.image.licenses="MIT OR Apache-2.0" \
      org.opencontainers.image.source="https://github.com/ineentho/safexip"
USER 10001
ENTRYPOINT ["/usr/bin/safexip"]
