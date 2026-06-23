---
id: batch
title: Batch & background tasks
---

# Batch & background tasks

`batch_fetch` validates every URL against the SSRF policy up front and hands back a `task_id` before a single request leaves the machine. The fetches run concurrently in the background. You watch progress through `rover task <id>` and its batch-specific alias `rover batch <id>`. Tasks survive a `rover mcp` restart.

## Fetching many URLs

`batch_fetch` returns immediately with a handle to a task, not page content. Pass between 1 and 100 URLs. Every URL is SSRF-validated before the task is scheduled, so one rejected address pre-empts the whole batch rather than failing item-by-item halfway through.

| Arg | Default | Range | What it controls |
| --- | --- | --- | --- |
| `urls` | — | 1–100 | The URLs to fetch. Each is SSRF-validated up front; any rejection pre-empts the task before it's scheduled. |
| `force_refresh` | `false` | — | Bypass the cache for every URL in the batch. |
| `concurrency` | `8` | clamped to `1..=32` | Total in-flight requests for the batch. |
| `per_domain_concurrency` | `2` | clamped to `1..=8` | In-flight requests per host. |

The response is a `TaskCreatedResponse`, returned the moment the task is inserted:

```jsonc
{
  "task_id": "0190c3a4-…",        // UUIDv7 — time-ordered
  "status": "running",
  "kind": "batch_fetch",
  "monitor_command": "rover batch 0190c3a4-… --monitor",
  "poll_command": "rover batch 0190c3a4-…",
  "cancel_command": "rover task 0190c3a4-… --cancel"
}
```

`concurrency` caps total in-flight requests; `per_domain_concurrency` caps how many of those can hit the same host. Leave both at their defaults and a batch spanning ten domains stays polite to each one while still saturating your overall budget. The values are clamped, not rejected — ask for `concurrency: 200` and you get 32.

## Warm the cache, then read

`batch_fetch` populates the cache with raw extracted content; it does not return guarded documents. Call `batch_fetch` to fill the cache, then `fetch` each URL to read it. The second call is a cache hit, and the full prompt-injection guard — wrapper, detectors, telemetry — runs on that `fetch`, the same as any other.

The guard runs transitively, on the read, not on the warm. The guarantee that protects your context window attaches when the content crosses into the model's view. A warmed page you never read was never guarded, because it was never handed to anyone. See [Trust & prompt injection](/docs/trust) for what the guard does and why the wrapper is the load-bearing part.

## Monitoring a task

`rover task <id>` is the universal reader: a snapshot of any task's progress and its latest event. `rover batch <id>` is the same command with one extra check — it asserts the task's `kind` is `batch_fetch` and errors if it isn't. Use `rover batch` when you know you're watching a batch and want the type guard; use `rover task` for anything Rover scheduled.

Both commands take the same flags:

| Flag | Effect |
| --- | --- |
| `--monitor` | Stream events as they're appended, instead of printing a single snapshot. |
| `--from-event <N>` | Resume a stream from event `N` — pair with `--monitor` to pick up an interrupted feed. |
| `--cancel` | Request cooperative cancellation by setting a flag (see below). |
| `--format human` | One readable line per event. The default. |
| `--format ndjson` | One JSON object per line — for scripting and log pipelines. |

These are pure readers. They touch the cache database, not the network — the only exception is `--cancel`, which writes a single flag. A streamed batch runs through `item_started` and `item_done` per URL (or `item_failed` on error), closing with `task_completed`. The terminal rollup records total, succeeded, failed, and duration, so a completed run tells you what it did without a re-scan.

```sh
rover batch <id> --monitor                         # live event stream until the task ends
rover task <id>                                    # one-shot snapshot of any task
rover task <id> --monitor --from-event 42          # resume an interrupted stream at event 42
rover batch <id> --format ndjson                   # snapshot as a single JSON line
```

Each NDJSON line is a self-contained JSON object, so you can pipe `--monitor --format ndjson` straight into `jq` and filter on `kind` without parsing prose. The `human` format is for a person watching a terminal; `ndjson` is for everything that isn't.

## Cancelling

Cancellation is cooperative. `rover task <id> --cancel` (or `rover batch <id> --cancel`) sets a flag; the worker checks it between items and stops scheduling new fetches. In-flight requests finish, the task records a terminal rollup, and the status moves to cancelled. A batch already mid-fetch on three URLs lets those three land before it winds down.

## Resilience

Tasks are persisted in SQLite, so they survive a `rover mcp` restart. Restart the server mid-batch and the job resumes from its persisted progress — URLs already recorded as done or failed are skipped, and the worker picks up where it left off. Nothing re-fetches because the process bounced.

Not every task can resume, and the ones that can't say so. A batch resumes cleanly because its progress is event-by-event. A summarisation job that can't pick up where it stopped is marked `failed` with a clear reason, so the agent can re-request rather than silently lose the work.

Task IDs are UUIDv7, so they're time-ordered. Sort a list of task IDs lexically and you get them in creation order — handy when reconciling a log of `task_id`s after the fact.

## Beyond batches

`rover task <id>` covers more than batches. Rover schedules background work for other reasons — stale-while-revalidate cache refreshes and deferred retries after a long `Retry-After` — and they're all tasks with the same monitor, poll, and cancel surface. When a `fetch` serves a stale copy and queues a revalidation, that revalidation is a task you can watch with `rover task <id>`, exactly like a batch. Only `rover batch` asserts the `kind`, which is the point of the alias.

See the [CLI reference](/docs/cli) for the full flag and exit-code surface, the [MCP tools](/docs/mcp-tools) reference for the `batch_fetch` schema, [Caching & freshness](/docs/caching) for how the warmed cache behaves on the later read, and [Configuration](/docs/configuration) for the rate-limit and concurrency defaults the scheduler inherits.
