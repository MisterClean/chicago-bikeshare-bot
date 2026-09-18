#!/usr/bin/python3
"""Install an approved native release; never restore or migrate the live DB."""
import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import sqlite3
import subprocess
import tarfile
import tempfile
import time
import urllib.request

ROOT = Path('/opt/chicago-bikeshare-bot')
STATE = Path('/var/lib/divvy-bot')  # Persistent compatibility path, intentionally unchanged.
DB = STATE / 'bot.sqlite3'
ENV = Path('/etc/divvy-bot.env')
REPO = 'MisterClean/chicago-bikeshare-bot'
SERVICE = 'chicago-bikeshare-bot.service'
ASSET = 'chicago-bikeshare-bot-linux-x86_64.tar.gz'
OLD_TIMERS = ('divvy-bot.timer', 'divvy-bot-deploy.timer', 'divvy-bot-health.timer')


def command(*args, **kwargs):
    return subprocess.run(args, check=True, **kwargs)


def active(unit):
    result = command('systemctl', 'show', '--property=ActiveState', '--value', unit, capture_output=True, text=True)
    # Type=oneshot stays "activating" for its entire execution.
    return result.stdout.strip() not in ('inactive', 'failed', '')


def lock_worker(lock):
    deadline = time.monotonic() + 1550
    while True:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            return
        except BlockingIOError:
            if time.monotonic() >= deadline:
                raise TimeoutError('Worker did not become idle; leaving it running')
            time.sleep(2)


def download(url, destination, limit):
    request = urllib.request.Request(url, headers={'User-Agent': 'chicago-bikeshare-bot-deployer', 'Cache-Control': 'no-cache'})
    with urllib.request.urlopen(request, timeout=60) as source, destination.open('wb') as output:
        count = 0
        while chunk := source.read(65536):
            count += len(chunk)
            if count > limit:
                raise ValueError('Release download exceeds size limit')
            output.write(chunk)


def validate_manifest(value):
    if not re.fullmatch('[0-9a-f]{40}', value.get('commit', '')):
        raise ValueError('Invalid release commit')
    if not re.fullmatch('[0-9a-f]{64}', value.get('sha256', '')):
        raise ValueError('Invalid release checksum')
    if value.get('asset') != ASSET or value.get('repository') != REPO:
        raise ValueError('Unexpected release repository or asset')
    return value


def extract(archive, destination):
    with tarfile.open(archive, 'r:gz') as bundle:
        members = bundle.getmembers()
        if sum(m.size for m in members) > 128 * 1024 * 1024:
            raise ValueError('Expanded release too large')
        for member in members:
            path = Path(member.name)
            if path.is_absolute() or '..' in path.parts or not (member.isfile() or member.isdir()):
                raise ValueError('Unsafe release archive member')
        bundle.extractall(destination, members=members, filter='data')
    for name in ('chicago-bikeshare-bot', 'pt', 'deploy/run-bot'):
        if not (destination / name).is_file():
            raise ValueError(f'Missing executable: {name}')


def snapshot(source, target):
    with sqlite3.connect(f'file:{source}?mode=ro', uri=True) as src, sqlite3.connect(target) as dst:
        src.backup(dst)


def history(path):
    with sqlite3.connect(f'file:{path}?mode=ro', uri=True) as db:
        return {
            'schema': db.execute("SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY name").fetchall(),
            'migrations': db.execute('SELECT * FROM schema_migrations ORDER BY version').fetchall(),
            'stations': dict(db.execute('SELECT id,first_seen_at FROM stations')),
            'events': dict(db.execute('SELECT id,payload FROM events')),
            'delivered': db.execute("SELECT * FROM deliveries WHERE status='delivered' ORDER BY id").fetchall(),
        }


def verify_history(before, after):
    if before['schema'] != after['schema'] or before['migrations'] != after['migrations']:
        raise ValueError('Schema or migration history changed')
    for category in ('stations', 'events'):
        if not before[category].items() <= after[category].items():
            raise ValueError(f'Historical {category} changed or disappeared')
    delivered = set(after['delivered'])
    if not all(row in delivered for row in before['delivered']):
        raise ValueError('A delivered historical record changed or disappeared')


def worker(candidate, database, publishing):
    # A second assignment in this private copy overrides production safely.
    # EnvironmentFile overrides systemd Environment/--setenv, so using --setenv
    # here could otherwise accidentally publish from the shadow database.
    with tempfile.NamedTemporaryFile(mode='w', prefix='bikeshare-env-', dir='/run') as env_file:
        env_file.write(ENV.read_text().rstrip() + '\n')
        env_file.write('DB_PATH=' + str(database) + '\n')
        if publishing is not None:
            env_file.write('PUBLISH_ENABLED=' + ('true' if publishing else 'false') + '\n')
        env_file.flush()
        unit = 'bikeshare-candidate-' + str(time.time_ns())
        try:
            command('systemd-run', '--unit=' + unit, '--quiet', '--wait', '--pipe', '--collect',
                '--property=User=ubuntu', '--property=Group=ubuntu',
                '--property=WorkingDirectory=/var/lib/divvy-bot',
                '--property=EnvironmentFile=' + env_file.name,
                '--property=MemoryMax=64M', '--property=MemorySwapMax=0',
                '--property=RuntimeMaxSec=1500', '--property=TimeoutStopSec=10',
                '--property=NoNewPrivileges=true',
                    str(candidate / 'chicago-bikeshare-bot'), 'run', timeout=1550)
        finally:
            subprocess.run(['systemctl', 'stop', unit], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def point_to(candidate):
    temporary = ROOT / 'current.next'
    temporary.unlink(missing_ok=True)
    temporary.symlink_to(candidate)
    temporary.replace(ROOT / 'current')


def install_support(candidate):
    for unit in (candidate / 'deploy/systemd').glob('*'):
        shutil.copy2(unit, Path('/etc/systemd/system') / unit.name)
    shutil.copy2(candidate / 'deploy/logrotate', '/etc/logrotate.d/chicago-bikeshare-bot')
    shutil.copy2(candidate / 'deploy/update.py', '/usr/local/sbin/deploy-chicago-bikeshare-bot')
    command('systemctl', 'daemon-reload')


def deploy(manifest, scratch):
    current = ROOT / 'current'
    previous = current.resolve() if current.exists() else None
    candidate = ROOT / 'releases' / manifest['commit']
    if previous == candidate:
        command('systemctl', 'start', SERVICE)
        command('systemctl', 'is-active', '--quiet', SERVICE)
        print('Already deployed and active ' + manifest['commit'], flush=True)
        return
    if not DB.is_file() or DB.stat().st_size == 0:
        raise ValueError('Existing production database is required')
    if shutil.disk_usage(ROOT).free < 256 * 1024 * 1024:
        raise ValueError('Less than 256 MiB disk space available')
    archive = scratch / ASSET
    download(f'https://github.com/{REPO}/releases/download/app-{manifest["commit"]}/{ASSET}', archive, 64 * 1024 * 1024)
    if hashlib.sha256(archive.read_bytes()).hexdigest() != manifest['sha256']:
        raise ValueError('Release checksum mismatch')
    unpacked = scratch / 'bundle'
    extract(archive, unpacked)
    command(str(unpacked / 'pt'), 'validate', str(unpacked / 'deploy/jobs'))
    command(str(unpacked / 'chicago-bikeshare-bot'), 'check', env={**os.environ, 'DB_PATH': str(DB), 'PUBLISH_ENABLED': 'false'})
    if candidate.exists():
        shutil.rmtree(candidate)
    shutil.move(unpacked, candidate)
    for path in candidate.rglob('*'):
        path.chmod(0o755 if path.is_dir() or os.access(path, os.X_OK) else 0o644)
    candidate.chmod(0o755)
    stopped = []
    switched = False
    with (STATE / 'deployment.lock').open('a') as lock:
        os.chown(lock.name, 1000, 1000)
        # Acquire before stopping Petit so no publishing worker is interrupted.
        lock_worker(lock)
        try:
            # One-time cutover: defer if the old worker/deployer is running.
            if active('divvy-bot.service') or active('divvy-bot-deploy.service'):
                raise RuntimeError('Legacy worker/deployer active; retry on next poll')
            for timer in OLD_TIMERS:
                if active(timer):
                    command('systemctl', 'stop', timer)
                    stopped.append(timer)
            # Check again after preventing new legacy runs.
            if active('divvy-bot.service') or active('divvy-bot-deploy.service'):
                raise RuntimeError('Legacy worker raced cutover; retry on next poll')
            command('systemctl', 'stop', SERVICE) if previous else None
            stamp = datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ')
            backups = STATE / 'native-backups'
            backups.mkdir(mode=0o700, exist_ok=True)
            backup = backups / (stamp + '.sqlite3')
            snapshot(DB, backup)
            before = history(backup)
            shadow = STATE / ('native-shadow-' + stamp + '.sqlite3')
            try:
                snapshot(backup, shadow)
                os.chown(shadow, 1000, 1000)
                worker(candidate, shadow, False)
                verify_history(before, history(shadow))
            finally:
                for suffix in ('', '-wal', '-shm'):
                    Path(str(shadow) + suffix).unlink(missing_ok=True)
                shadow.with_suffix('.sqlite3.run.lock').unlink(missing_ok=True)
            point_to(candidate)
            switched = True
            install_support(candidate)
            # Honor the existing publishing flag; never silently enable posting.
            worker(candidate, DB, None)
            verify_history(before, history(DB))
            fcntl.flock(lock, fcntl.LOCK_UN)
            command('systemctl', 'enable', '--now', SERVICE)
            command('systemctl', 'is-active', '--quiet', SERVICE)
            command('systemctl', 'enable', '--now', 'chicago-bikeshare-bot-health.timer')
            (ROOT / 'deployed.json').write_text(json.dumps({**manifest, 'deployed_at': stamp, 'previous': str(previous) if previous else None}) + '\n')
            for timer in OLD_TIMERS:
                command('systemctl', 'disable', timer)
        except Exception:
            lock_worker(lock)
            if switched:
                command('systemctl', 'stop', SERVICE)
                if not previous:
                    command('systemctl', 'disable', SERVICE)
                if previous:
                    point_to(previous)
                    install_support(previous)
                else:
                    current.unlink(missing_ok=True)
            # Restore scheduling, never restore database bytes after live writes.
            fcntl.flock(lock, fcntl.LOCK_UN)
            if previous:
                command('systemctl', 'start', SERVICE)
            for timer in stopped:
                command('systemctl', 'enable', '--now', timer)
            raise
        finally:
            fcntl.flock(lock, fcntl.LOCK_UN)
    # Keep rollback releases and seven consistent pre-deployment backups.
    keep = {candidate, previous}
    releases = sorted((ROOT / 'releases').iterdir(), key=lambda p: p.stat().st_mtime, reverse=True)
    for old in releases[3:]:
        if old not in keep:
            shutil.rmtree(old)
    for old in sorted((STATE / 'native-backups').glob('*.sqlite3'), reverse=True)[7:]:
        old.unlink()
    print('Deployed ' + manifest['commit'] + '; historical data verified', flush=True)


def interrupted(signum, frame):
    raise InterruptedError(f'Deployment interrupted by signal {signum}')


def main():
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    if os.geteuid() != 0:
        raise SystemExit('Run this deployer as root')
    ROOT.mkdir(mode=0o755, parents=True, exist_ok=True)
    (ROOT / 'releases').mkdir(exist_ok=True)
    with Path('/run/lock/chicago-bikeshare-bot-deploy.lock').open('a') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return
        with tempfile.TemporaryDirectory(prefix='download-', dir=ROOT) as directory:
            scratch = Path(directory)
            manifest_path = scratch / 'production.json'
            download(f'https://github.com/{REPO}/releases/download/production/production.json', manifest_path, 16384)
            deploy(validate_manifest(json.loads(manifest_path.read_text())), scratch)


if __name__ == '__main__':
    main()
