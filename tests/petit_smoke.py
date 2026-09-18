"""Exercise the real pinned scheduler, failure records and child timeout safely."""
import os
from pathlib import Path
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time

binary = str(Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory() as directory:
    root = Path(directory)
    jobs = root / 'jobs'
    jobs.mkdir()
    for job, command, args, timeout in [('success', '/bin/echo', '[ok]', 5), ('failure', '/usr/bin/false', '[]', 5), ('timeout', '/bin/sleep', '["30"]', 1)]:
        (jobs / (job+'.yaml')).write_text(f'''id: {job}
name: {job}
schedule:
  cron: "* * * * * *"
  timezone: UTC
max_concurrency: 1
tasks:
  - id: execute
    type: command
    command: {command}
    args: {args}
    timeout_secs: {timeout}
    retry:
      max_attempts: 0
      delay_secs: 0
      condition: never
''')
    subprocess.run([binary, 'validate', str(jobs)], check=True)
    db_path = root/'history.sqlite3'
    with (root/'scheduler.log').open('w') as log:
        child = subprocess.Popen([binary, 'run', str(jobs), '--db', str(db_path), '-j', '3', '-t', '1'], stdout=log, stderr=log, start_new_session=True)
        try:
            deadline = time.monotonic()+20
            found = set()
            while time.monotonic() < deadline:
                if child.poll() is not None:
                    raise AssertionError((root/'scheduler.log').read_text())
                if db_path.exists():
                    try:
                        with sqlite3.connect(db_path) as db:
                            found = set(db.execute('SELECT job_id,status FROM runs'))
                    except sqlite3.OperationalError:
                        pass
                if {('success','completed'), ('failure','failed'), ('timeout','failed')} <= found:
                    break
                time.sleep(0.2)
            else:
                raise AssertionError(f'Expected terminal states, got {found}: '+(root/'scheduler.log').read_text())
            with sqlite3.connect(db_path) as db:
                assert db.execute("SELECT count(*) FROM task_states WHERE status='failed'").fetchone()[0] >= 2
            print('Petit scheduled success, failure and timeout recorded correctly')
        finally:
            os.killpg(child.pid, signal.SIGINT)
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait()
