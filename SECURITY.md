# Security Policy

## Reporting a vulnerability

Report security issues privately to **aaronbassett@gmail.com**. Do not open a
public issue for anything that could put users at risk.

Please include enough detail to reproduce: affected version or commit, the
configuration in play, and a proof of concept if you have one.

## What to expect

Rover is maintained by a single person. There is no formal SLA.

- **Acknowledgement:** I aim to confirm receipt within 7 days.
- **Triage:** once acknowledged, I will let you know whether the report is
  accepted, needs more information, or is out of scope.
- **Fix and disclosure:** timelines depend on severity and my availability. I
  will keep you updated and credit you in the changelog unless you prefer to
  stay anonymous.

## Supported versions

Rover is pre-1.0 and has no tagged releases yet. Only the `main` branch is
supported — fixes land there and nowhere else.

| Version | Supported |
| ------- | --------- |
| `main`  | Yes       |

This will change once `0.1.0` ships, at which point this table will track
supported release lines.

## Threat model

Rover fetches and processes untrusted web content, and its threat model is not
obvious. Before reporting, read [`docs/security.md`](docs/security.md) — it
documents the assets Rover protects, the adversaries it defends against, and the
known limitations that are accepted by design (for example, opt-in HAR debug
output is intentionally not redacted).
