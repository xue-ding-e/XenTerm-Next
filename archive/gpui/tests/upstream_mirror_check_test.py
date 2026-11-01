"""Exercise the actual Bash checker against isolated local Git repositories."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / '.github/scripts/sync-upstream.sh'


class MirrorCheckTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.upstream = self.root / 'upstream'
        self.mirror = self.root / 'mirror'
        self.git('init', '-q', '--initial-branch=main', str(self.upstream))
        self.commit('base')
        self.git('clone', '-q', '--bare', str(self.upstream), str(self.mirror))
        self.git('-C', str(self.mirror), 'branch', 'upstream-main', 'main')
        self.env = os.environ.copy()
        # Redirect only these public fixture URLs; no network or credentials.
        self.env.update({
            'GIT_CONFIG_COUNT': '2',
            'GIT_CONFIG_KEY_0': f'url.{self.mirror}.insteadOf',
            'GIT_CONFIG_VALUE_0': 'https://github.com/xue-ding-e/XenTerm-Next.git',
            'GIT_CONFIG_KEY_1': f'url.{self.upstream}.insteadOf',
            'GIT_CONFIG_VALUE_1': 'https://github.com/ixbaicn/XenTerm.git',
            'GIT_CONFIG_NOSYSTEM': '1',
            'GIT_CONFIG_GLOBAL': os.devnull,
            'GIT_TERMINAL_PROMPT': '0',
            'GITHUB_STEP_SUMMARY': str(self.root / 'summary'),
        })

    def git(self, *args):
        return subprocess.check_output(['git', *args], text=True, stderr=subprocess.PIPE).strip()

    def commit(self, name):
        self.git('-C', str(self.upstream), '-c', 'user.name=Fixture',
                 '-c', 'user.email=fixture@example.invalid',
                 'commit', '-q', '--allow-empty', '-m', name)

    def run_check(self, expected_refs=None):
        paths = [self.upstream, self.mirror]
        before = [self.git('-C', str(p), 'show-ref') for p in paths]
        result = subprocess.run(['bash', str(SCRIPT)], env=self.env,
                                capture_output=True, text=True, timeout=30)
        self.assertEqual(before if expected_refs is None else expected_refs,
                         [self.git('-C', str(p), 'show-ref') for p in paths])
        return result

    def assert_concurrent_change_fails(self, phase, message):
        base = self.git('-C', str(self.upstream), 'rev-parse', 'HEAD')
        self.commit('concurrent update')
        newer = self.git('-C', str(self.upstream), 'rev-parse', 'HEAD')
        # Make the new object available before the simulated concurrent writer runs.
        self.git('-C', str(self.mirror), 'fetch', '-q', str(self.upstream),
                 'main:race-object')
        self.git('-C', str(self.upstream), 'reset', '--hard', base)
        upstream_changes = phase == 'after_upstream_fetch'
        repo = self.upstream if upstream_changes else self.mirror
        ref = 'refs/heads/main' if upstream_changes else 'refs/heads/upstream-main'
        expected_refs = [self.git('-C', str(p), 'show-ref')
                         for p in [self.upstream, self.mirror]]
        index = 0 if upstream_changes else 1
        expected_refs[index] = expected_refs[index].replace(
            f'{base} {ref}', f'{newer} {ref}')

        real_git = shutil.which('git')
        self.assertIsNotNone(real_git)
        bin_dir = self.root / 'bin'
        bin_dir.mkdir()
        marker = self.root / 'concurrent-writer-ran'
        call_log = self.root / 'git-calls.jsonl'
        wrapper = bin_dir / 'git'
        # Run real Git first, then let an independent writer move exactly one ref.
        # Calls from the checker are logged; the writer bypasses the wrapper.
        wrapper.write_text(f'''#!/usr/bin/env python3
import json
import os
import subprocess
import sys

args = sys.argv[1:]
with open({str(call_log)!r}, 'a') as log:
    log.write(json.dumps(args) + '\\n')
returncode = subprocess.call([{real_git!r}] + args)
phase = {phase!r}
mirror_url = 'https://github.com/xue-ding-e/XenTerm-Next.git'
upstream_url = 'https://github.com/ixbaicn/XenTerm.git'
trigger = (
    phase == 'after_mirror_read' and args == [
        'ls-remote', '--exit-code', mirror_url, 'refs/heads/upstream-main']
    or phase == 'after_mirror_fetch' and args[2:] == [
        'fetch', '--quiet', '--no-tags', mirror_url, 'refs/heads/upstream-main']
    or phase == 'after_upstream_fetch' and args[2:] == [
        'fetch', '--quiet', '--no-tags', upstream_url, 'refs/heads/main']
)
if returncode == 0 and trigger and not os.path.exists({str(marker)!r}):
    open({str(marker)!r}, 'w').close()
    subprocess.check_call([{real_git!r}, '-C', {str(repo)!r},
                           'update-ref', {ref!r}, {newer!r}, {base!r}])
sys.exit(returncode)
''')
        wrapper.chmod(0o755)
        self.env['PATH'] = str(bin_dir) + os.pathsep + os.environ['PATH']
        result = self.run_check(expected_refs)
        self.assertTrue(marker.exists(), 'The concurrent writer was not triggered')
        self.assertNotEqual(result.returncode, 0, result.stderr)
        self.assertIn(message, result.stdout)
        self.assertNotIn('Status:', result.stdout)
        self.assertIn(message, (self.root / 'summary').read_text())
        calls = [json.loads(line) for line in call_log.read_text().splitlines()]
        for args in calls:
            self.assertNotIn('push', args, 'The checker attempted a remote write')
            self.assertNotIn('update-ref', args, 'The checker attempted a ref write')

    def test_current_is_read_only(self):
        result = self.run_check()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('Status: current.', result.stdout)
        self.assertIn('no synchronization is performed', (self.root / 'summary').read_text())

    def test_behind_reports_pending_without_push(self):
        self.commit('upstream update')
        result = self.run_check()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('behind by 1 commit(s)', result.stdout)
        self.assertIn('Synchronization is pending', (self.root / 'summary').read_text())
        self.assertIn('::warning::', result.stdout)

    def test_divergence_fails_without_mutating_refs(self):
        self.commit('mirror-only change')
        self.git('-C', str(self.mirror), 'fetch', '-q', str(self.upstream), 'main:upstream-main')
        self.git('-C', str(self.upstream), 'reset', '--hard', 'HEAD~1')
        self.commit('different upstream change')
        result = self.run_check()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Status: diverged', result.stdout)

    def test_missing_mirror_fails_closed(self):
        self.git('-C', str(self.mirror), 'branch', '-D', 'upstream-main')
        result = self.run_check()
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn('Status: current.', result.stdout)

    def test_failed_fetch_does_not_report_current(self):
        self.env['GIT_CONFIG_KEY_1'] = f'url.{self.root / "missing"}.insteadOf'
        result = self.run_check()
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn('Status: current.', result.stdout)

    def test_mirror_change_during_fetch_fails_closed(self):
        self.assert_concurrent_change_fails(
            'after_mirror_read', 'Mirror changed during fetch')

    def test_mirror_change_after_fetch_fails_closed(self):
        self.assert_concurrent_change_fails(
            'after_mirror_fetch', 'Mirror changed concurrently')

    def test_upstream_change_after_fetch_fails_closed(self):
        self.assert_concurrent_change_fails(
            'after_upstream_fetch', 'Official main changed concurrently')

    def test_workflow_is_read_only_and_runs_this_suite(self):
        workflow = (ROOT / '.github/workflows/sync-upstream.yml').read_text()
        self.assertIn('contents: read', workflow)
        self.assertIn('persist-credentials: false', workflow)
        self.assertNotIn('contents: write', workflow)
        self.assertNotIn('secrets.', workflow)
        self.assertIn('python3 tests/upstream_mirror_check_test.py', workflow)
        self.assertIn('push:', workflow)
        self.assertIn('pull_request:', workflow)
        self.assertNotIn('git push', SCRIPT.read_text())


if __name__ == '__main__':
    unittest.main(verbosity=2)
