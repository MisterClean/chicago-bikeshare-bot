#!/usr/bin/python3
"""Read bot health and prune only the separate Petit execution history."""
import datetime
import json
import shutil
import sqlite3
import subprocess
import time
from pathlib import Path

subprocess.run(['systemctl', 'is-active', '--quiet', 'chicago-bikeshare-bot.service'], check=True)
with sqlite3.connect('file:/var/lib/divvy-bot/bot.sqlite3?mode=ro', uri=True) as db:
    if db.execute('PRAGMA quick_check').fetchone()[0] != 'ok':
        raise SystemExit('Bot database integrity check failed')
    queue = dict(db.execute('SELECT status,count(*) FROM deliveries GROUP BY status'))
    print(json.dumps({'delivery_counts': queue}))
    stale = db.execute("SELECT count(*) FROM deliveries WHERE status != 'delivered' AND julianday('now') - julianday(created_at) > 1").fetchone()[0]
    if stale:
        raise SystemExit(f'{stale} undelivered records older than 24 hours')
    last = db.execute("SELECT max(started_at) FROM runs WHERE status='succeeded'").fetchone()[0]
    if not last or datetime.datetime.now(datetime.timezone.utc) - datetime.datetime.fromisoformat(last.replace('Z', '+00:00')) > datetime.timedelta(hours=8):
        raise SystemExit('No successful bot run within eight hours')
    latest = db.execute('SELECT status FROM runs ORDER BY started_at DESC LIMIT 1').fetchone()[0]
    if latest == 'failed':
        raise SystemExit('Latest bot run failed; inspect worker.log')
    print(json.dumps({'last_success': last, 'latest_status': latest}))
if shutil.disk_usage('/var/lib/divvy-bot').free < 256 * 1024 * 1024:
    raise SystemExit('Less than 256 MiB free disk space')
path = Path('/var/lib/petit-chicago-bikeshare-bot/history.sqlite3')
if path.exists():
    with sqlite3.connect(path) as db:
        cutoff = int((time.time() - 90 * 86400) * 1000)
        # Petit stores milliseconds as text. No cleanup ever targets bot.sqlite3.
        predicate = "CAST(started_at AS INTEGER) < ? AND status NOT IN ('pending','running')"
        db.execute(f'DELETE FROM task_states WHERE run_id IN (SELECT id FROM runs WHERE {predicate})', (cutoff,))
        db.execute(f'DELETE FROM runs WHERE {predicate}', (cutoff,))
