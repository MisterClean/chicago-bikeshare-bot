#!/usr/bin/python3
"""Called only before the sole scheduler starts, after systemd reaps its cgroup."""
import sqlite3
import time
from pathlib import Path

path = Path('/var/lib/petit-chicago-bikeshare-bot/history.sqlite3')
if path.exists():
    with sqlite3.connect(path) as db:
        now = str(int(time.time() * 1000))
        db.execute("UPDATE task_states SET status='failed', ended_at=?, error='Scheduler restarted' WHERE run_id IN (SELECT id FROM runs WHERE status IN ('pending','running')) AND status IN ('pending','running')", (now,))
        db.execute("UPDATE runs SET status='interrupted', ended_at=?, error='Scheduler restarted' WHERE status IN ('pending','running')", (now,))
