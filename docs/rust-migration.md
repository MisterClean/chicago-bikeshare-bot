# Current deployment decision (2026-09-18)

The authorized final deployment is native Rust with pinned Petit, replacing the
initial container plan below. See [the current runbook](../deploy/README.md).
GitHub repository/application name: `chicago-bikeshare-bot`. The existing
credential and database paths stay in place; only the Bluesky identifier changes
to `chi-bike-stations.bsky.social`. No historical database migration is performed.

The following sections preserve the original read-only audit and proposal.

# Rust migration plan and acceptance gates

Production inspection on 2026-09-18 was read-only. No deployment is authorized in this phase.

## Findings

The active application is TypeScript v2. Python is an unused predecessor. The Lightsail host has 412 MiB RAM and 2 GiB swap (~794 MiB occupied while the bot is idle). The renderer launches Chromium/software WebGL and downloads three full CTA datasets per card. Replacing only Node would leave this major resource cost.

Existing strengths to preserve: silent first baseline, complete-feed validation, atomic station/event reconciliation, durable retries, stable AT Protocol record keys, alternating persisted card styles, optional Street View replies, and image-digest rollback.

The live database is `/var/lib/divvy-bot/bot.sqlite3`, owned by UID 1000, bind-mounted at the same container path. It has migration versions 1 and 2 and the same schema as this checkout. Inspection found 1,357 historical stations, 30 events and 30 delivered records; the latest feed contained 1,210 stations. Missing stations MUST remain in history. `/etc/divvy-bot.env` stays host-owned and untouched. Production uses 1080×1350, pixel ratio 1, zooms 17/17.5, Protomaps key and Street View.

## Implementation sequence

1. Single-threaded Rust executable with bounded blocking HTTP, typed configuration, SQLite prepared statements and one delivery at a time. Keep current environment names and CLI commands. Add explicitly read-only `status`/`check` and safe local preview support.
2. Retain schema v2 without migration, import, reset, vacuum, timestamp rewriting or rebaselining on existing databases. Only ordinary run/reconciliation/delivery updates occur during `run`. Refuse unknown/older schemas; only initialize a genuinely empty new DB. Preserve legacy import as an explicit command on empty targets.
3. Preserve event JSON and old record keys, including the TypeScript reply-clock calculation. Look up existing posts before rendering/uploading on retry. Bound network time/bytes; failures requeue. Serialize runs with an OS file lock in the DB directory.
4. Replace browser rendering with native CPU drawing, retain civic/nightline identity, real map geometry and attribution, and verify representative output visually. Native cartography retains the card identity but changes MapLibre label placement and styling. No browser, Node or Python in production.
5. Regression tests against a fixture with the actual v2 SQL schema and historical records; mocked source/AT Protocol tests, queue/retry/recovery/rollback tests, complete-feed rejection, image bounds and memory measurement in release mode.
6. Multi-stage Rust Docker image, same UID 1000 and default one-shot command. Update CI; retain the release and host deployment contract. Do not merge, push production tags, SSH-write files, or invoke deployment during local development.

## Release gate

Before merge: cargo fmt, clippy with warnings denied, tests, release build, visual checks, measured release peak RSS, and Linux container smoke test where a Docker daemon is available. Document unavailable checks accurately.

Merge to main triggers CI, then immutable GHCR build, then production environment approval by MisterClean before production-tag promotion. The existing host deployer polls the tag, stops the timer, creates a consistent backup plus a disposable shadow DB, runs the candidate against the shadow with publishing disabled, checks schema compatibility, switches only the image pointer and starts one live cycle. It restores the old image pointer on failure; it must not restore stale DB data over delivered records. The production environment and DB paths remain identical. Local code must not depend on installing updated host scripts.

The candidate should pass read-only `check` on a local snapshot before cutover. Preserve the current image digest for rollback. After authorized deployment verify integrity, original historical rows/record keys, queue counts, logs, RSS and successful run. A rollback changes the image pointer, never replaces the live database.

## Audit limitations and operational findings

No historical Chromium peak RSS or OOM evidence was retained; map timeouts are confirmed but cannot be attributed to memory alone. At audit, the health watchdog was failing because free disk (~2.7 GiB) was below its 3 GiB threshold. The deployer requires 2 GiB. No cleanup was performed. Production had 89 failed and 144 successful historical runs; the latest failure was August 27. Existing deployment/health scripts match the checkout.

Current rollback reference: `ghcr.io/misterclean/divvy-bluesky-bot@sha256:3d2c46c62e80f248d76e4cedd567f6c3e51bb2fd20ddbcd7e1e1977b3eb61f07` (commit `f908d9f324e5c87b67cc46c6ce6edb853cdbc4bd`).

Reference contracts: [Protomaps basemap layers](https://docs.protomaps.com/basemaps/layers), [Socrata spatial filtering](https://dev.socrata.com/docs/functions/intersects), [AT Protocol TIDs](https://atproto.com/specs/tid), [Bluesky posting](https://docs.bsky.app/docs/tutorials/creating-a-post).

## Completed locally

The native Rust rewrite, fixture/mock tests, real map previews, v2 snapshot
compatibility comparison and `linux/amd64` container checks are complete. Both
a no-change run and map render passed in a 64 MiB container without swap.
See [verification](verification.md) for exact measurements and limitations.
No merge, push or production write has occurred. Final review should include
the changed cartography in the native card previews before release approval.
