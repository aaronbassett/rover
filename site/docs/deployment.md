---
id: deployment
title: Deployment
---

# Deployment

Run Rover as a container on a shared network and every agent that can reach it gets the same cache — a page fetched by one agent is a cache hit for the next. `rover mcp --http` is what makes this possible: a long-running HTTP server multiple clients call concurrently, in place of the one-process-per-agent stdio transport. See [`rover mcp --http`](/docs/cli) for the flag and endpoint reference and [`[http]`](/docs/configuration#http) for the config block; this page covers running it as a container.

## Compose

The repository ships a `docker-compose.yml` that runs the image on an internal network with no host port published. Agent containers reach Rover at `http://rover:7683/mcp`; nothing else on the host can.

```yaml
# Rover on an internal network. No host port is published: agent containers
# reach it at http://rover:7683/mcp and nothing binds on the host.
services:
  rover:
    build: .
    environment:
      # Fails the whole `up` if unset, rather than starting an open instance.
      ROVER_HTTP_TOKEN: "${ROVER_HTTP_TOKEN:?set ROVER_HTTP_TOKEN in .env — generate with: openssl rand -hex 32}"
      ROVER_CONFIG: /config/rover.toml
      ANTHROPIC_API_KEY: ${ANTHROPIC_API_KEY:-}
      RUST_LOG: info
    volumes:
      - rover-data:/data
      - ./rover.toml:/config/rover.toml:ro
    networks: [agents]
    # Bounded, not Docker's unbounded default: an unauthenticated caller on
    # this network can drive hundreds of rejected requests/sec against
    # /mcp, and without a cap that fills the host disk. See "Security
    # limits" below.
    logging:
      driver: json-file
      options: { max-size: "10m", max-file: "3" }

volumes:
  rover-data:

networks:
  agents:
```

Two required steps before the first `up` — compose does not do either for you, and skipping the first fails silently rather than loudly:

1. **Create `rover.toml` from the example.** `docker-compose.yml` bind-mounts `./rover.toml`, which the repository does not ship (it's your local, possibly environment-specific config). If that path doesn't exist on the host, Docker creates a *directory* there instead of failing, Rover finds an unreadable, non-file config path, and silently falls back to built-in defaults — your `[http]` and `[ssrf]` settings never apply and nothing tells you why.

   ```bash
   cp rover.toml.example rover.toml
   ```

2. **Generate the token**, or compose refuses to start:

   ```bash
   echo "ROVER_HTTP_TOKEN=$(openssl rand -hex 32)" >> .env
   ```

The image's own `CMD` already runs `rover mcp --http --bind 0.0.0.0:7683`, so the compose file doesn't repeat it — `build: .` plus the environment above is the whole setup.

## The volume

`/data` holds the SQLite database (cache entries, task state, the event log), downloaded tokenizers, and any extracted output (image downloads, table CSVs). It has to survive restarts: an empty volume means a cold cache and a fresh tokenizer download on every `docker compose up`.

Docker seeds a named volume's content and ownership from the image only the first time that volume is created. Two consequences:

- A `rover-data` volume created by an image built before this ownership fix stays root-owned. Upgrading onto it fails with `EACCES` the moment Rover tries to open `rover.db` as uid `10001`. Run `docker volume rm rover-data` once and let the next `up` recreate it from the current image.
- Bind mounts are never seeded and always keep host ownership, regardless of image. If you bind-mount `/data` instead of using a named volume, `chown 10001:10001` it on the host first.

## Client configuration

Point an MCP client at the container's `/mcp` path with the token in an `Authorization` header:

```json
{
  "mcpServers": {
    "rover": {
      "type": "http",
      "url": "http://rover:7683/mcp",
      "headers": { "Authorization": "Bearer ${ROVER_HTTP_TOKEN}" }
    }
  }
}
```

## Server-side paths

`tables.mode = "csv_file"` and `images.mode = "download"` write files to `/data` on the server and put the absolute path in the returned Markdown — a path that means nothing to a caller in a different container. Both are refused over HTTP by default. Set `[http] allow_server_paths = true` only when the client mounts the same volume at the same path, so the path it receives actually resolves on its own filesystem.

## First run needs outbound network

The first `count_tokens` call, or the first token-budgeted `fetch`, downloads a tokenizer from HuggingFace into `/data/.cache/huggingface/hub`. It happens once per volume — after that, the tokenizer is cached alongside everything else under `/data` and every later call is offline. If the container has no outbound network access, that first call fails.

## SPA rendering

The default image ships without Chromium: it is built without the `headless` Cargo feature, so `headless.mode = "on"` returns `headless_feature_not_compiled` and `auto` behaves like a plain fetch. See [JavaScript & dynamic pages](/docs/dynamic-pages) for the mode-by-mode behaviour.

A second build target adds it:

```bash
docker build --target runtime-headless -t rover:headless .
```

Or use the overlay, which sets the flags below for you:

```bash
docker compose -f docker-compose.headless.yml up
```

That image is about 1.1GB against the default's 83MB, almost entirely Chromium and its dependencies.

### It needs three run flags

Chrome's sandbox does not start under Docker's default seccomp profile, which blocks the user namespaces it relies on. Running it needs:

- `--security-opt seccomp=chrome.json` — the profile is in the repository root
- `--shm-size=1g` — Chrome crashes on non-trivial pages with Docker's 64MB `/dev/shm`
- a non-root user — already set in the image; do not override it

Without the profile, Rover refuses to render and tells you so. Chrome's own error output suggests `--no-sandbox` as a workaround. Do not take it: it removes the boundary that keeps a hostile page inside the renderer process, and Rover renders whatever page an agent asks for.

### Rebuilding is how you get security fixes

The image installs Chromium from `bookworm-security` at build time and deliberately does not pin the version, so each rebuild picks up Debian's current backport. Nothing pushes a new image to you — Rover ships a Dockerfile, not a registry image — so the browser's patch level is a function of how often you rebuild. Chrome sandbox escapes are exploited in the wild; an image built once and left alone drifts to a known-vulnerable browser.

## Request limits

A request body over 16 MiB is rejected. With a `Content-Length` header — every real MCP client sends one — that's a clean `413`. A chunked request with no `Content-Length` is truncated at the same limit but comes back as `500`, because the oversize collect error surfaces from the JSON-RPC layer rather than the body-limit layer. Memory use is bounded either way; only the status code differs.

## Security limits

The bearer token is compared in constant time, but nothing rate-limits or locks out repeated failed attempts — a short or predictable `ROVER_HTTP_TOKEN` is brute-forceable at line rate. Generate a real one:

```bash
openssl rand -hex 32
```

That's a deliberate gap, not an oversight: the deployment target is a trusted container network, and closing it is only defensible if an operator can actually see abuse happening. Two things make that possible. Every rejection carries the caller's address (`peer = <addr>`) so you know who to look at, and the line itself is throttled to at most once per second with a `suppressed` count folded in — an unauthenticated caller hammering `/mcp` produces one log line a second, not one per request, so the log itself can't become the disk-filling attack (`docker-compose.yml`'s `logging:` limits are the second half of that same mitigation). Neither of those slows an attacker down; they only make the attempt visible.

Treat the container network as the primary boundary, not the token — put Rover on a network only trusted callers can reach, the way the compose file above does, and let the token be a second layer rather than the only one.

## Health

`GET /healthz` is liveness and returns the running Rover version; `GET /readyz` opens the cache database and returns `200` if it answers, `503` if it doesn't. Neither requires `ROVER_HTTP_TOKEN`, so an orchestrator can probe both with no credential configured.

`/healthz/` and `/readyz/` — trailing slash — are different, unrouted paths, not the same endpoint with looser matching. Once a token is set, every unrouted path returns `401` instead of `404`, so a probe pointed at the wrong URL fails as an auth error rather than a connection error. Configure your orchestrator's probe against the exact path, no trailing slash.

The image has no shell, so there's no in-container `HEALTHCHECK` — Docker's `healthcheck:` needs a command to run inside the container, and distroless doesn't have one. The shipped compose file has no `healthcheck:` block for the same reason: probe `/readyz` from your orchestrator, the way it already reaches the container over the network.
