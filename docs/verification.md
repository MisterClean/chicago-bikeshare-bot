# Rust refactor verification

Work performed locally on 2026-09-18. Production was inspected over SSH
read-only. No deployment, service restart, environment-file edit, live DB write,
or Bluesky post was performed.

## Production compatibility

A consistent read-only production snapshot was obtained only after confirming
the bot service was inactive and its WAL was empty. The remote connection used
SQLite `immutable=1`, serialized directly to local storage; no remote snapshot
file was created. Credential values were not logged. Local render checks used
the existing Protomaps key in process memory without editing `.env`.

The Rust `check` command validated the copied database: 1,357 stations and 30
delivered records. A publishing-disabled run against a separate disposable copy
fetched 1,210 source stations with zero discovered/electrified events. SQL
comparisons confirmed:

- All 30 event rows were identical.
- All 30 delivery rows, including record keys and posted URIs/CIDs, were identical.
- Both migration rows and the complete SQL schema were identical.
- All 1,357 station IDs and first-seen timestamps were identical.
- Missing historical stations were retained; only ordinary observed-state and
  run-history updates occurred.

## Automated checks

32 tests pass. `cargo fmt --check`, `cargo clippy --all-targets --all-features
--locked -- -D warnings`, `cargo test --locked`, and the release build pass.

Tests include real SQLite transactions and rollback, fixture v2 schema, a
historical camelCase event with a large string station ID, preserved delivery
keys/styles, delayed retries and abandoned claims, process locking, source
rejection, full mocked root/reply publication, uncertain-create recovery,
failed-reply recovery without re-rendering the root, nonfatal Street View,
publication-time timestamps, native geometry/card rendering and JPEG limits.

No test uses production Bluesky credentials or sends live posts. The PDS login,
upload, create and lookup interactions are tested against local HTTP servers.

## Visual verification and memory

Both native styles were rendered with real Protomaps tiles and geographically
filtered CTA data at State St & Randolph St. A south-side station was also
checked. The image dimensions are 1080×1350, files fit the Bluesky size limit,
and visual inspection checked the headings, marker, route overlays, text and
attribution. Native label placement and cartographic styling differ from the
previous browser renderer.

The following migration-baseline measurements precede the data-notice changes below.
Measurements use the release executable, not Cargo, through `/usr/bin/time -l`
on macOS/Apple Silicon. They measure the single worker process; there is no
browser child. The no-change shadow run peaked at 13.0 MiB RSS and took 1.1 s.
The final detailed map runs peaked at 33.1 MiB (civic) and 32.4 MiB (nightline) RSS and took 5.4 s and 4.7 s respectively.
These are local measurements, not Lightsail benchmarks. No retained production
Chromium peak is available, so a percentage RAM reduction is not claimed.

## Container verification

The initial attempt to run x86 rustc under local CPU emulation crashed before
compiling the app. The Dockerfile now uses the builder's native Rust compiler
and the target's cross linker. The target runtime remains `linux/amd64`, UID
1000, the same database mount and environment names, and default command `run`.
The migration-baseline `linux/amd64` container built successfully. Initialization and `check`
passed as UID/GID 1000 with a 96 MiB cap. A live-source, publishing-disabled
shadow run and a real native map render both passed with **64 MiB RAM and swap
disabled**:

| Workload | Linux cgroup peak |
| --- | ---: |
| No-change production-copy shadow run | 26,198,016 bytes (25.0 MiB) |
| 1080×1350 civic map with real tiles and CTA | 40,996,864 bytes (39.1 MiB) |

These cgroup measurements include the shell and local x86 emulation overhead.
They are not native Lightsail measurements. The completed runtime image is
35,189,174 bytes (33.6 MiB) according to Docker image inspection, compared with
419,513,842 bytes (400.1 MiB) for the inspected production image. Its config
identifies `linux/amd64`, user `1000:1000`, entrypoint `divvy-bot`, command `run`.
The tested executable/config digest was
`sha256:3640d587581ba7fbf7442a2ab800d5ba879e4e1de9a48b93ac21e5bdcb3b52ca`.
No code executes through Node, Python, Chromium or a GPU in this container.

## Data-notice follow-up

After the terms review, reran formatting, Clippy, all 32 tests, the native
release build, and the `linux/amd64` container build. The added regression
covers long Unicode station names with the source/availability notice retained
within the post length limit. Removed the logo-only SVG dependency and bundled
the data notice and font license in the runtime image.

Both updated card styles were rendered with real data and inspected. The new
civic/nightline native runs peaked at 36,601,856 and 33,587,200 bytes RSS. The
updated container rendered the civic card with a 64 MiB cap and swap disabled;
cgroup peak was 42,393,600 bytes (40.4 MiB). The image is 34,869,012 bytes
(33.3 MiB), with local image ID
`sha256:5b97c9bc170c2052a6c4cf3d832b4f546b5e828353a1165e043780202dda72e6`.
No new shadow reconciliation was needed: these changes do not touch detection,
schema, historical state, or event keys. The production snapshot SHA-256 remains
`5ed089801e8a3194c212ea591d302d27d618ba53e35117df10b92e69800822c7`.
The public profile was read but not changed. No posts were sent.

## Image wording and layout revision

Restored OPEN/DEPLOYED/CHARGED with Bikeshare Station headings, removed the
top-left badge, and combined credits into one line. Formatting, Clippy, the
existing card-rendering test, and the native release build pass. Both live-data
native previews were regenerated and visually checked for headings, badge
removal, and a readable single-line footer. Container measurements above
precede this presentation-only revision.

## Release boundaries

Nothing has been pushed, merged or promoted. Merge to main runs CI/build;
GitHub's production environment requires MisterClean's approval before tag
promotion. The current host poller backs up and shadow-tests a copy, then
switches only the image pointer. Its shadow test does not exercise rendering or
publishing, which is why the separate local tests above are required.

The existing disk-space health failure (2.7 GiB free versus 3 GiB required) is
unrelated to this refactor and was not changed. Production memory and first-run
logs still need observation after an explicitly authorized deployment.

## Native deployment and rename (2026-09-18)

The application/package/executable and remote repository are now named
`chicago-bikeshare-bot`. Source-system attribution retains the proper name Divvy;
legacy production database and credential paths deliberately remain unchanged.
The logged-in Bluesky profile shows **Chicago Bikeshare Stations** at
`chi-bike-stations.bsky.social`. Authentication with the existing app password
confirmed the unchanged DID `did:plc:iczo3amdbvcbhxikgzhiaxzj`. Only the production
`BLUESKY_IDENTIFIER` line was changed; every other credential/config line was
compared byte-for-byte. A root-only backup includes the original environment,
old systemd units/scripts and a consistent SQLite snapshot. Before cutover:
1,357 stations, 30 events, 30 deliveries and 233 run-history records.

The final deployment uses pinned Petit and native release bundles rather than
the Docker deployment originally proposed. Rust formatting, Clippy and all 32
worker tests pass after the rename. Eight deployment safety tests cover archive
traversal/special-file rejection, manifest validation, historical-state invariants,
uncheckpointed SQLite backups, secret-free shadow environment overrides,
atomic release pointers, running one-shot detection and rollback after scheduler
startup failure without reverting committed live database writes.

The actual pinned Petit executable was exercised with temporary scheduled jobs:
a successful command, nonzero exit and timeout all produced the expected
persisted states. CI repeats this against Linux executables. The API and TUI
are omitted. Full worker output is redirected to rotated persistent logs;
Petit history is separate and terminal records expire after 90 days.

The updater preserves production environment semantics for live runs and appends
`PUBLISH_ENABLED=false` to a private temporary copy for shadow runs. This avoids
systemd's EnvironmentFile overriding command-line environment settings. It waits
for idle workers, verifies candidate state on a copy, and rolls back executables
and scheduling on errors or termination. It never automatically restores the
historical bot database.
