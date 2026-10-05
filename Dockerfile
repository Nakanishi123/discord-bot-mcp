FROM docker.io/library/rust:1.99.0-trixie AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY src ./src
RUN cargo build --release --locked

FROM gcr.io/distroless/cc-debian13:nonroot
COPY --from=builder /app/target/release/discord-bot-mcp /usr/local/bin/discord-bot-mcp
COPY LICENSE /usr/share/licenses/discord-bot-mcp/LICENSE
USER 65532:65532
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/discord-bot-mcp"]
