# Compile on the builder's native CPU, including on Apple Silicon. Only the
# finished executable uses the requested platform; rustc never needs emulation.
FROM --platform=$BUILDPLATFORM rust:1.94.0-slim-bookworm AS build
ARG TARGETARCH
WORKDIR /build
RUN case "$TARGETARCH" in \
      amd64) target=x86_64-unknown-linux-gnu; compiler=gcc-x86-64-linux-gnu; headers=libc6-dev-amd64-cross ;; \
      arm64) target=aarch64-unknown-linux-gnu; compiler=gcc-aarch64-linux-gnu; headers=libc6-dev-arm64-cross ;; \
      *) echo "Unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac \
    && if [ "$(dpkg --print-architecture)" = "$TARGETARCH" ]; then compiler=gcc; headers=libc6-dev; fi \
    && rustup target add "$target" \
    && apt-get update \
    && apt-get install --no-install-recommends -y "$compiler" "$headers" libc6-dev \
    && rm -rf /var/lib/apt/lists/*
ENV CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc
ENV CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
COPY Cargo.toml Cargo.lock ./
COPY rust ./rust
COPY assets ./assets
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/build/target \
    case "$TARGETARCH" in \
      amd64) target=x86_64-unknown-linux-gnu ;; \
      arm64) target=aarch64-unknown-linux-gnu ;; \
    esac \
    && cargo build --release --locked --target "$target" \
    && cp "target/$target/release/chicago-bikeshare-bot" /chicago-bikeshare-bot

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install --no-install-recommends -y ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 1000 bot \
    && useradd --uid 1000 --gid 1000 --no-create-home bot \
    && mkdir -p /app/data /app/output /var/lib/chicago-bikeshare-bot \
    && chown -R 1000:1000 /app /var/lib/chicago-bikeshare-bot
WORKDIR /app
COPY --from=build /chicago-bikeshare-bot /usr/local/bin/chicago-bikeshare-bot
COPY NOTICE.md /usr/share/doc/chicago-bikeshare-bot/NOTICE.md
COPY assets/fonts/OFL.txt /usr/share/doc/chicago-bikeshare-bot/OFL.txt
USER 1000:1000
ENTRYPOINT ["chicago-bikeshare-bot"]
CMD ["run"]
