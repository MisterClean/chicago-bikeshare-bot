# Native Lightsail deployment

The application and scheduler are Rust executables. GitHub builds them on Ubuntu
24.04 x86_64; production does not need Docker, Cargo, Node, or a Python package
environment. Small deployment, startup and health scripts use Ubuntu's built-in
Python 3 standard library.

## Persistent layout

| Location | Purpose |
| --- | --- |
| `/opt/chicago-bikeshare-bot/releases/<commit>/` | Root-owned executable bundles; three recent releases plus rollback target retained. |
| `/opt/chicago-bikeshare-bot/current` | Atomic pointer to active bundle. |
| `/etc/divvy-bot.env` | Existing root-only credentials; preserved legacy path. |
| `/var/lib/divvy-bot/bot.sqlite3` | Existing v2 historical database; never replaced or automatically migrated. |
| `/var/lib/divvy-bot/native-backups/` | Seven consistent pre-deployment backups, root-only directory. |
| `/var/lib/petit-chicago-bikeshare-bot/history.sqlite3` | Separate Petit execution history, terminal records retained 90 days. |
| `/var/log/chicago-bikeshare-bot/worker.log` | Persistent worker output, rotated daily or at 5 MiB, 14 rotations. |

The legacy paths are deliberate: changing the app name does not rename its live
state. `BLUESKY_IDENTIFIER=chi-bike-stations.bsky.social` identifies the same
account DID, `did:plc:iczo3amdbvcbhxikgzhiaxzj`. No app-password rotation or post
history migration is required. `.env` and databases never enter release bundles.

## CI/CD

The required `test` CI job runs Rust formatting, Clippy, tests, deployment safety
tests, a native release build and real Petit schedule/failure/timeout checks.
It uploads the exact tested bundle. Successful **push CI on main** invokes
Release, gated by the existing **production environment approval**. Release
attests the bundle, publishes `app-<commit>`, and updates the `production`
release's `production.json` manifest. The environment reviewer gate remains in
place; future deployments require that approval before the host receives them.

A five-minute systemd timer downloads the public approved manifest over HTTPS.
The deployer pins the repository, validates commit/checksum formats, verifies
SHA-256, rejects unsafe archives, and validates the candidate binaries/jobs.
No GitHub token or incoming SSH credential is required for normal updates.
Provenance attestations are available on GitHub; host verification uses the
approved HTTPS manifest plus SHA-256, not an on-host Sigstore verifier.

Deployment waits for the worker lock before stopping the scheduler. It creates
a consistent SQLite backup, runs the candidate against a disposable copy with
publishing forced off, and compares the schema, migrations, historical IDs,
first-seen timestamps, event payloads, and delivered records. It then switches
the executable pointer, runs the live worker under a 64 MiB limit, verifies the
same invariants, and starts Petit. Failed cutovers restore the previous executable
and scheduling. **Rollback never restores database bytes after live writes.**

The original Docker units and image are kept on the host for the initial
migration fallback. After success, their three timers are disabled. They are
not used for new deployments. Other applications and their PM2 processes remain
independent.

## CLI operations

```sh
# Scheduler status/start/stop/restart and logs
sudo systemctl status chicago-bikeshare-bot
sudo systemctl restart chicago-bikeshare-bot
sudo journalctl -u chicago-bikeshare-bot -n 50

# One immediate run, with production credentials and the deployment lock
sudo systemctl start chicago-bikeshare-bot-run
sudo tail -n 50 /var/log/chicago-bikeshare-bot/worker.log

# Read-only application inspection (credentials are unnecessary)
DB_PATH=/var/lib/divvy-bot/bot.sqlite3 /opt/chicago-bikeshare-bot/current/chicago-bikeshare-bot status
DB_PATH=/var/lib/divvy-bot/bot.sqlite3 /opt/chicago-bikeshare-bot/current/chicago-bikeshare-bot check

# Inspect/validate Petit jobs
/opt/chicago-bikeshare-bot/current/pt list /opt/chicago-bikeshare-bot/current/deploy/jobs
/opt/chicago-bikeshare-bot/current/pt validate /opt/chicago-bikeshare-bot/current/deploy/jobs

# Poll for an approved release immediately
sudo systemctl start chicago-bikeshare-bot-deploy
sudo journalctl -u chicago-bikeshare-bot-deploy -n 80
```

Petit runs every six hours at 00:00, 06:00, 12:00 and 18:00 **UTC**. A startup
check runs immediately when the most recent scheduled slot has no successful
bot run. The wrapper uses `exec`, so task timeout targets the actual worker.
Systemd owns the entire cgroup, starts Petit at boot and limits the scheduler
plus worker to 96 MiB with no swap. Worker output goes directly to its rotated
file, avoiding Petit's in-memory stdout buffering. Partial logs survive timeouts.

Petit is pinned to `170dee43be847b29bc065f4265a4b3e16d4fb7fd`, built with only
`sqlite` (no HTTP API or TUI). Its upstream documentation calls it experimental.
Its `pt trigger` starts a separate scheduler and does **not** propagate task
failure as an exit failure; use the systemd manual-run service above. Startup
marks stale Petit executions interrupted after systemd has reaped old children.
The bot's own durable queue/recovery is independent of Petit history.

Health runs every 30 minutes, checks service state, the last bot run, a successful
run within eight hours, and at least 256 MiB disk headroom. Failures are visible
in `systemctl --failed` and the journal. There is no external alert integration.

## Bootstrap and rollback

Bootstrap requires Ubuntu 24.04 x86_64, Python 3.12+, systemd, CA certificates,
`flock`, and the existing UID 1000 `ubuntu` account and production state.
Install root-owned `update.py` as `/usr/local/sbin/deploy-chicago-bikeshare-bot`,
create the log/Petit directories owned by ubuntu, install the two deploy unit
files, then enable `chicago-bikeshare-bot-deploy.timer`. The first approved
manifest completes the cutover, including the remaining units and logrotate.

To roll back a native release, dispatch **Roll back production** with the full
commit SHA of an existing `app-<commit>` release and approve its production gate.
This promotes that release through the same shadow/live checks. Schema changes
must be designed and approved separately; this deployer never migrates the DB.
Do not delete persistent state or run `init`/`import-legacy` against production.
