"""Regression tests for irreversible deployment failure modes; no network/prod access."""
import copy
import importlib.util
import io
from pathlib import Path
import sqlite3
import tarfile
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('updater', Path(__file__).parents[1] / 'deploy/update.py')
updater = importlib.util.module_from_spec(spec)
spec.loader.exec_module(updater)


class DeploymentSafety(unittest.TestCase):
    def test_archive_rejects_parent_traversal_and_symlinks(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name, kind in [('../outside', tarfile.REGTYPE), ('link', tarfile.SYMTYPE), ('device', tarfile.CHRTYPE)]:
                with self.subTest(name=name):
                    archive = root / 'bad.tar.gz'
                    with tarfile.open(archive, 'w:gz') as bundle:
                        member = tarfile.TarInfo(name)
                        member.type = kind
                        member.linkname = '/etc/passwd'
                        bundle.addfile(member, io.BytesIO())
                    with self.assertRaises(ValueError):
                        updater.extract(archive, root / 'unpacked')

    def test_manifest_rejects_foreign_repository_and_invalid_hash(self):
        good = dict(repository=updater.REPO, asset=updater.ASSET, commit='a'*40, sha256='b'*64)
        self.assertEqual(updater.validate_manifest(good), good)
        for key, value in [('repository', 'other/repo'), ('commit', '../escape'), ('sha256', 'invalid'), ('asset', 'other.tar.gz')]:
            with self.subTest(key=key), self.assertRaises(ValueError):
                updater.validate_manifest({**good, key: value})

    def test_history_check_rejects_loss_mutation_and_schema_change(self):
        before = dict(schema=[('table', 'stations')], migrations=[(1,), (2,)], stations={'old': 'first-seen'}, events={'event': 'payload'}, delivered=[('id', 'delivered', 'uri')])
        for category in before:
            with self.subTest(category=category):
                after = copy.deepcopy(before)
                after[category] = {} if isinstance(after[category], dict) else []
                with self.assertRaises(ValueError):
                    updater.verify_history(before, after)
        after = copy.deepcopy(before)
        after['stations']['new'] = 'now'
        after['events']['new'] = 'new payload'
        after['delivered'].append(('new', 'delivered', 'new-uri'))
        updater.verify_history(before, after)

    def test_snapshot_includes_uncheckpointed_wal(self):
        with tempfile.TemporaryDirectory() as directory:
            source, target = Path(directory)/'live.db', Path(directory)/'backup.db'
            with sqlite3.connect(source) as writer:
                writer.execute('PRAGMA journal_mode=WAL')
                writer.execute('CREATE TABLE history(value)')
                writer.execute("INSERT INTO history VALUES ('preserve me')")
                writer.commit()
                updater.snapshot(source, target)
                with sqlite3.connect(target) as backup:
                    self.assertEqual(backup.execute('SELECT * FROM history').fetchall(), [('preserve me',)])

    def test_shadow_env_disables_publication_without_changing_credentials(self):
        with tempfile.TemporaryDirectory() as directory:
            env = Path(directory)/'environment'
            original = 'BLUESKY_APP_PASSWORD=sentinel-secret\nPUBLISH_ENABLED=true\nDB_PATH=/live.db\n'
            env.write_text(original)
            original_tempfile = tempfile.NamedTemporaryFile
            def temporary(**kwargs):
                return original_tempfile(**{**kwargs, 'dir': directory})
            def inspect(*args, **kwargs):
                argument = next(a for a in args if a.startswith('--property=EnvironmentFile='))
                private = Path(argument.split('=', 2)[2])
                self.assertEqual(private.stat().st_mode & 0o777, 0o600)
                self.assertTrue(private.read_text().endswith('DB_PATH=/shadow.db\nPUBLISH_ENABLED=false\n'))
                self.assertNotIn('sentinel-secret', ' '.join(args))
            with patch.object(updater, 'ENV', env), patch.object(updater, 'command', inspect), patch.object(updater.subprocess, 'run'), patch.object(updater.tempfile, 'NamedTemporaryFile', temporary):
                updater.worker(Path('/release'), Path('/shadow.db'), False)
            self.assertEqual(env.read_text(), original)
            self.assertEqual(list(Path(directory).iterdir()), [env])

    def test_atomic_pointer_switch_preserves_prior_release(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(updater, 'ROOT', Path(directory)):
            old, new = Path(directory)/'old', Path(directory)/'new'
            old.mkdir(); new.mkdir()
            updater.point_to(old)
            updater.point_to(new)
            self.assertEqual((Path(directory)/'current').resolve(), new.resolve())
            self.assertTrue(old.exists())

    def test_scheduler_start_failure_restores_previous_release_without_rewinding_db(self):
        import contextlib
        import hashlib
        import subprocess
        with tempfile.TemporaryDirectory() as directory, contextlib.ExitStack() as stack:
            root = Path(directory)
            state, releases, scratch = root/'state', root/'releases', root/'scratch'
            for path in (state, releases, scratch):
                path.mkdir()
            previous = releases/'previous'
            previous.mkdir()
            (root/'current').symlink_to(previous)
            live = state/'bot.sqlite3'
            live.write_text('original history')
            environment = root/'environment'
            environment.write_text('PUBLISH_ENABLED=false')
            for key,value in [('ROOT',root), ('STATE',state), ('DB',live), ('ENV',environment)]:
                stack.enter_context(patch.object(updater,key,value))
            stack.enter_context(patch.object(updater.os,'chown'))
            stack.enter_context(patch.object(updater,'active',return_value=False))
            stack.enter_context(patch.object(updater,'download',side_effect=lambda url,path,limit:path.write_bytes(b'bundle')))
            def unpack(archive,path):
                path.mkdir()
                (path/'chicago-bikeshare-bot').write_text('binary')
            stack.enter_context(patch.object(updater,'extract',side_effect=unpack))
            stack.enter_context(patch.object(updater,'snapshot',side_effect=lambda source,target:target.write_text(source.read_text())))
            stack.enter_context(patch.object(updater,'history',return_value={}))
            stack.enter_context(patch.object(updater,'verify_history'))
            stack.enter_context(patch.object(updater,'install_support'))
            def run_worker(candidate,db,publishing):
                if db == live:
                    db.write_text('original history plus committed delivery')
            stack.enter_context(patch.object(updater,'worker',side_effect=run_worker))
            calls=[]
            def execute(*args,**kwargs):
                calls.append(args)
                if args == ('systemctl','enable','--now',updater.SERVICE):
                    raise subprocess.CalledProcessError(1,args)
            stack.enter_context(patch.object(updater,'command',side_effect=execute))
            manifest=dict(commit='a'*40,sha256=hashlib.sha256(b'bundle').hexdigest())
            with self.assertRaises(subprocess.CalledProcessError):
                updater.deploy(manifest,scratch)
            self.assertEqual((root/'current').resolve(),previous.resolve())
            self.assertEqual(live.read_text(),'original history plus committed delivery')
            self.assertIn(('systemctl','start',updater.SERVICE),calls)
            self.assertFalse((root/'deployed.json').exists())

    def test_oneshot_activating_is_treated_as_running(self):
        import subprocess
        with patch.object(updater,'command',return_value=subprocess.CompletedProcess([],0,stdout='activating\n')):
            self.assertTrue(updater.active('legacy.service'))


if __name__ == '__main__':
    unittest.main()
