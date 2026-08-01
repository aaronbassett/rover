# syntax=docker/dockerfile:1

# ----- chef base -----
# Pinned to the MSRV in Cargo.toml (rust-version = "1.96.0"). Bump both together.
FROM rust:1.96-bookworm AS chef
WORKDIR /app
# cargo-chef 0.1.68's vendored `cargo-manifest` (v0.14.0) predates Edition
# 2024 support and rejects this crate's `edition = "2024"` with "data did
# not match any variant of untagged enum MaybeInherited". 0.1.77 pulls in
# cargo-manifest v0.20.0, which parses it fine — verified against this repo.
RUN cargo install cargo-chef --locked --version 0.1.77

# ----- planner -----
# Needs the real source tree: `cargo chef prepare` shells out to `cargo
# metadata`, which resolves the explicit [lib]/[[bin]] paths in Cargo.toml.
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ----- builder -----
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# Only this layer is invalidated by a source change that leaves deps alone.
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin rover

# The runtime stage is distroless: no shell, so mkdir/chown are impossible
# there. Create the data directory here with the right ownership and copy it
# across, otherwise Docker creates the named volume's mount point root-owned
# and the uid-10001 process fails with EACCES opening rover.db.
RUN mkdir -p /data && chown -R 10001:10001 /data

# ----- builder (headless) -----
# Separate from `builder` so a default `docker build .` never compiles
# chromiumoxide. `cargo chef cook` is re-run with the feature because the
# default recipe does not cover chromiumoxide's dependency tree.
FROM chef AS builder-headless
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --features headless --recipe-path recipe.json
COPY . .
RUN cargo build --release --features headless --bin rover
RUN mkdir -p /data && chown -R 10001:10001 /data

# ----- runtime (headless) -----
# NOT distroless: distroless has no package manager, and this stage needs
# Chromium. Must appear BEFORE `runtime` so the final stage — and therefore
# the default build target — stays the small image.
FROM debian:bookworm-slim AS runtime-headless

# Deliberately unpinned. Resolves against bookworm-security at build time, so
# each rebuild picks up Debian's current Chromium backport; pinning would
# freeze a known-vulnerable browser. `chromium-sandbox` is deliberately NOT
# installed: it does not make the sandbox work under Docker's default seccomp
# profile, and it adds a setuid-root binary. See the design doc.
RUN apt-get update \
 && apt-get install -y --no-install-recommends chromium ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# distroless ships uid 10001 implicitly; bookworm-slim does not.
RUN useradd -u 10001 -m -d /home/rover rover

COPY --from=builder-headless /app/target/release/rover /usr/local/bin/rover
COPY --from=builder-headless --chown=10001:10001 /data /data

ENV ROVER_DATA_DIR=/data
# See the note on the `runtime` stage: HOME, not HF_HOME. Same panic applies.
ENV HOME=/data

# chrome_executable is left unset: chromiumoxide's PATH lookup includes
# "chromium" (detection.rs:69), which resolves Debian's /usr/bin/chromium.
# Step 4 verifies this rather than assuming it.

VOLUME /data
EXPOSE 7683
USER 10001:10001

ENTRYPOINT ["/usr/local/bin/rover"]
CMD ["mcp", "--http", "--bind", "0.0.0.0:7683"]

# ----- runtime -----
FROM gcr.io/distroless/cc-debian12 AS runtime

COPY --from=builder /app/target/release/rover /usr/local/bin/rover
COPY --from=builder --chown=10001:10001 /data /data

ENV ROVER_DATA_DIR=/data

# HOME, not HF_HOME. Rover's tokenizer download calls `ApiBuilder::new()`
# (src/tokenizer/download.rs:37), which uses `Cache::default()` —
# `dirs::home_dir().expect("Cache directory cannot be found")`
# (hf-hub-0.4.3/src/lib.rs:197-205). HF_HOME is read only by
# `Cache::from_env()`, which this path never calls, so setting HF_HOME would
# be ignored. With no HOME and uid 10001 absent from /etc/passwd,
# `home_dir()` returns None and the first tokenizer download PANICS.
ENV HOME=/data

VOLUME /data
EXPOSE 7683
USER 10001:10001

ENTRYPOINT ["/usr/local/bin/rover"]
CMD ["mcp", "--http", "--bind", "0.0.0.0:7683"]
