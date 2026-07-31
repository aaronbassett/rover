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

volumes:
  rover-data:

networks:
  agents:
```

Generate the token before the first `up`, or compose refuses to start:

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

## No SPA rendering

The image is built without the `headless` Cargo feature, so it ships without Chromium. `headless.mode = "on"` returns the `headless_feature_not_compiled` error rather than a silent fallback; `auto` and the default just behave like a plain fetch. See [JavaScript & dynamic pages](/docs/dynamic-pages) for the full mode-by-mode behaviour.

That's a deliberate omission, not a missing feature. Rover renders whatever page an agent asks for, and that page is untrusted by definition — running Chrome in a container without its sandbox, the usual workaround for "browser in Docker," hands a hostile page more room to move than not rendering it at all. If you need SPA rendering, run the `headless`-featured binary directly, on a host where Chrome's sandbox stays intact.

## Request limits

A request body over 16 MiB is rejected. With a `Content-Length` header — every real MCP client sends one — that's a clean `413`. A chunked request with no `Content-Length` is truncated at the same limit but comes back as `500`, because the oversize collect error surfaces from the JSON-RPC layer rather than the body-limit layer. Memory use is bounded either way; only the status code differs.

## Security limits

The bearer token is compared in constant time, but nothing throttles the comparison itself: there is no rate limiting or lockout on repeated failed attempts, and no throttling on the per-rejection log line. A short or predictable `ROVER_HTTP_TOKEN` is brute-forceable at line rate. Generate a real one:

```bash
openssl rand -hex 32
```

Treat the container network as the primary boundary, not the token — put Rover on a network only trusted callers can reach, the way the compose file above does, and let the token be a second layer rather than the only one.

## Health

`GET /healthz` is liveness and returns the running Rover version; `GET /readyz` opens the cache database and returns `200` if it answers, `503` if it doesn't. Neither requires `ROVER_HTTP_TOKEN`, so an orchestrator can probe both with no credential configured.

`/healthz/` and `/readyz/` — trailing slash — are different, unrouted paths, not the same endpoint with looser matching. Once a token is set, every unrouted path returns `401` instead of `404`, so a probe pointed at the wrong URL fails as an auth error rather than a connection error. Configure your orchestrator's probe against the exact path, no trailing slash.

The image has no shell, so there's no in-container `HEALTHCHECK` — Docker's `healthcheck:` needs a command to run inside the container, and distroless doesn't have one. The shipped compose file has no `healthcheck:` block for the same reason: probe `/readyz` from your orchestrator, the way it already reaches the container over the network.
