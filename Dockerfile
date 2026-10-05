# syntax=docker/dockerfile:1
FROM rust:1.98.1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e AS build

WORKDIR /app
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY vendor ./vendor
RUN cargo build --release --locked -p rigspark-cli --bin llmup --bin rigspark -p rigspark-gui --bin rigspark-gui

# Debian 13 (glibc 2.41) runs both the source build (bookworm, glibc 2.36) and the
# release-archive binaries, which are built on Ubuntu 24.04 and need glibc 2.39.
FROM debian:trixie-slim@sha256:a99cfc517144bc59b1978475ec53b46ecabec7e43635402ee5b77cc54cd1b20a AS base

WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates lsof procps \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system llmup \
    && useradd --system --gid llmup --create-home --home-dir /home/llmup llmup

ENV HOME=/home/llmup
ENV RIGSPARK_HOME=/home/llmup/.rigspark

USER llmup
ENTRYPOINT ["/usr/local/bin/llmup"]
CMD ["recommend", "--json"]

# Published images: the checksum-verified binaries and notices from this platform's
# release archive, extracted into release-bin/ by .github/workflows/container.yml.
FROM base AS release
COPY release-bin/llmup release-bin/rigspark release-bin/rigspark-gui /usr/local/bin/
COPY release-bin/LICENSE release-bin/THIRD-PARTY.md release-bin/CROSSTERM-PATCH.md release-bin/*.LICENSE release-bin/*.LICENSE.md /usr/share/doc/rigspark/

# Default target: build the binaries from source.
FROM base AS runtime
COPY --from=build /app/target/release/llmup /usr/local/bin/llmup
COPY --from=build /app/target/release/rigspark /usr/local/bin/rigspark
COPY --from=build /app/target/release/rigspark-gui /usr/local/bin/rigspark-gui
COPY LICENSE crates/rigspark-gui/vendor/README.md crates/rigspark-gui/vendor/marked.LICENSE.md crates/rigspark-gui/vendor/dompurify.LICENSE /usr/share/doc/rigspark/
COPY vendor/crossterm/LICENSE /usr/share/doc/rigspark/crossterm.LICENSE
COPY vendor/crossterm/RIGSPARK-PATCH.md /usr/share/doc/rigspark/CROSSTERM-PATCH.md
