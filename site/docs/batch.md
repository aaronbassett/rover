---
id: batch
title: Batch & background tasks
---

# Batch & background tasks

`batch_fetch` validates every URL against the SSRF policy before scheduling, then returns a `task_id`. No request leaves the machine until the whole batch passes. The fetches run concurrently in the background, and you watch progress through `rover task <id>` or its batch-specific alias `rover batch <id>`. Tasks are persisted, so they survive a `rover mcp` restart.

## Fetching many URLs

`batch_fetch` returns a task handle, not page content. Pass between 1 and 100 URLs. SSRF validation happens up front for every URL, so a single rejected address pre-empts the entire batch instead of failing item-by-item halfway through.

| Arg | Default | Range | What it controls |
| --- | --- | --- | --- |
| `urls` | none | 1-100 | The URLs to fetch. Each is SSRF-validated up front; any rejection pre-empts the task before it's scheduled. |
| `force_refresh` | `false` | n/a | Bypass the cache for every URL in the batch. |
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

`concurrency` caps total in-flight requests. `per_domain_concurrency` caps how many of those can hit one host. At the defaults, a batch spanning ten domains stays polite to each one while still saturating the overall budget. Both values are clamped rather than rejected, so a request for `concurrency: 200` runs at 32.

## Warm the cache, then read

`batch_fetch` populates the cache with raw extracted content. It does not return guarded documents. Warm the cache with `batch_fetch`, then call `fetch` on each URL to read it. The second call is a cache hit, and the full prompt-injection guard (wrapper, detectors, telemetry) runs on that `fetch`, exactly as it would on any other.

The guard runs transitively, on the read rather than the warm. The guarantee that protects your context window attaches when content crosses into the model's view. A page you warm but never read was never guarded, because it was never handed to anyone. See [Trust & prompt injection](/docs/trust) for what the guard does and why the wrapper is the load-bearing part.

## Monitoring a task

`rover task <id>` is the universal reader. It prints a snapshot of any task's progress and its latest event. `rover batch <id>` runs the same command with one extra check: it asserts the task's `kind` is `batch_fetch` and errors otherwise. Reach for `rover batch` when you know you're watching a batch and want the type guard. Use `rover task` for anything Rover scheduled.

Both commands take the same flags:

| Flag | Effect |
| --- | --- |
| `--monitor` | Stream events as they're appended, instead of printing a single snapshot. |
| `--from-event <N>` | Resume a stream from event `N`; pair with `--monitor` to pick up an interrupted feed. |
| `--cancel` | Request cooperative cancellation by setting a flag (see below). |
| `--format human` | One readable line per event. The default. |
| `--format ndjson` | One JSON object per line, for scripting and log pipelines. |

These are pure readers. They touch the cache database, never the network. The one exception is `--cancel`, which writes a single flag. A streamed batch emits `item_started` and `item_done` per URL (or `item_failed` on error), then closes with `task_completed`. The terminal rollup records total, succeeded, failed, and duration, so a finished run reports what it did without a re-scan.

```sh
rover batch <id> --monitor                         # live event stream until the task ends
rover task <id>                                    # one-shot snapshot of any task
rover task <id> --monitor --from-event 42          # resume an interrupted stream at event 42
rover batch <id> --format ndjson                   # snapshot as a single JSON line
```

Every NDJSON line is a self-contained JSON object, so you can pipe `--monitor --format ndjson` straight into `jq` and filter on `kind` without parsing prose. Use `human` when a person is watching the terminal and `ndjson` for everything else.

## Cancelling

Cancellation is cooperative. `rover task <id> --cancel` (or `rover batch <id> --cancel`) sets a flag. The worker checks it between items and stops scheduling new fetches. In-flight requests finish, the task records a terminal rollup, and the status moves to cancelled. Cancel a batch already mid-fetch on three URLs and those three land before it winds down.

## Resilience

Tasks are persisted in SQLite, so they survive a `rover mcp` restart. Restart the server mid-batch and the job resumes from its persisted progress: URLs already recorded as done or failed are skipped, and the worker picks up where it left off. A bounced process triggers no re-fetches.

Not every task can resume, and the ones that can't say so. A batch resumes cleanly because its progress is recorded event-by-event. A summarisation job that can't pick up where it stopped is marked `failed` with a clear reason, so the agent can re-request instead of silently losing the work.

Task IDs are UUIDv7, so they're time-ordered. Sort a list of task IDs lexically and they come out in creation order, which is handy when reconciling a log of `task_id`s after the fact.

## Beyond batches

`rover task <id>` covers more than batches. Rover schedules background work for other reasons too, like stale-while-revalidate cache refreshes and deferred retries after a long `Retry-After`. All of them are tasks with the same monitor, poll, and cancel surface. When a `fetch` serves a stale copy and queues a revalidation, that revalidation is a task you can watch with `rover task <id>`, exactly like a batch. Only `rover batch` asserts the `kind`, which is the whole point of the alias.

See the [CLI reference](/docs/cli) for the full flag and exit-code surface, the [MCP tools](/docs/mcp-tools) reference for the `batch_fetch` schema, [Caching & freshness](/docs/caching) for how the warmed cache behaves on the later read, and [Configuration](/docs/configuration) for the rate-limit and concurrency defaults the scheduler inherits.
