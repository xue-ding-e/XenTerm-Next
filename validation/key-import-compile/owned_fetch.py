"""Same-run locked Cargo acquisition through the already-qualified FetchHost.

Disk sampling stops acquisition on observed reserve/cache violations; it is NOT
an OS quota, instantaneous byte cap, or network allowlist. Unknown lifecycle
fails closed: no compile may follow. No product/build-script execution is asked
of Cargo fetch. Historical qualification does not prove current cache readiness.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import time
from adapter.compile_adapter import plain, require, digest, read_json
from acquisition_inputs import reject_unreviewed_config, inspect_cache_budget
from dependency_plan import public_locked_fetch_plan
from dependency_executor import summarize_fetch_receipt

HOST_FILES = frozenset('FetchHost' + suffix for suffix in ('.exe', '.dll', '.deps.json', '.runtimeconfig.json'))
OUTER_SECONDS = 615
POLL_SECONDS = 1
CACHE_SCAN_SECONDS = 5


from resource_monitor import monitor_host


class OwnedFetch:
    """Single-use launcher wired only by the reviewed fixed-source controller.

    Its build record is produced by that same run's build stage, not an arbitrary
    readiness boolean. Both historical evidence parsers remain read-only.
    """
    def __init__(self, root, host_directory, build_record, control_sha, run_id,
                 identity_gate, environment, transport_gate, host_entry_gate, host_sources):
        self.root = plain(root); self.host_directory = plain(host_directory)
        self.build_record = plain(build_record); self.control_sha = control_sha; self.run_id = str(run_id)
        self.identity_gate = identity_gate; self.environment = environment
        self.transport_gate = transport_gate; self.host_entry_gate = host_entry_gate
        self.host_sources = plain(host_sources)
        self.used = False

    def identity(self):
        require(os.name == 'nt' and os.environ.get('RUNNER_ENVIRONMENT') == 'github-hosted', 'hosted Windows fetch required')
        require(os.environ.get('GITHUB_REPOSITORY') == 'xue-ding-e/XenTerm-Next'
                and os.environ.get('GITHUB_SHA') == self.control_sha
                and os.environ.get('GITHUB_RUN_ID') == self.run_id, 'current fetch run identity')
        # Concrete pinned parsers, installed by the fixed controller, authenticate
        # both transport failure cases and actual Host-entry execution separately.
        self.transport_gate(); self.host_entry_gate(); self.identity_gate()
        require(self.host_directory.is_relative_to(self.root), 'fetch Host outside owned root')
        record = read_json(self.build_record, 32768)
        require(record.get('state') == 'compiled' and type(record.get('compile_exit')) is int
                and record['compile_exit'] == 0 and record.get('control_sha') == self.control_sha
                and str(record.get('run_id')) == self.run_id, 'fetch Host build provenance')
        require(all(record.get(k) is True for k in ('initial_identity', 'final_identity', 'compiled_copy_identity')),
                'fetch Host build identity')
        sources = {p.name: digest(plain(p)) for p in self.host_sources.iterdir() if p.is_file()}
        require(set(sources) == {'Program.cs', 'fetch-owned-process.cs', 'FetchHost.csproj', 'NuGet.Config', 'global.json'}
                and record.get('host_sources') == sources, 'fetch Host source snapshot')
        files = record.get('files', {})
        require(isinstance(files, dict) and set(files) == HOST_FILES, 'fetch Host runtime inventory')
        for name, expected in files.items():
            require(digest(plain(self.host_directory / name)) == expected, 'fetch Host binary changed')
        return files

    def __call__(self, plan):
        require(not self.used, 'fetch root already consumed')
        self.identity()
        require(public_locked_fetch_plan(self.root, self.identity_gate) == plan, 'fetch plan changed')
        reject_unreviewed_config(self.root)
        inspect_cache_budget(self.root, shutil.disk_usage(self.root).free)
        # The caller must pass the exact environment validated by FetchHost; no
        # parent env is merged here. Native validation occurs before Cargo starts.
        request = {key: plan[key] for key in ('executable', 'args', 'cwd', 'seconds', 'commandPolicy', 'completionMode')}
        request.update(environment=self.environment, stdout=str(self.root / 'fetch.stdout.log'),
                       stderr=str(self.root / 'fetch.stderr.log'))
        raw = json.dumps(request).encode(); require(len(raw) <= 131072, 'fetch request bound')
        for name in ('fetch.request.json', 'fetch.receipt.json', 'fetch-started.marker', 'fetch.stdout.log', 'fetch.stderr.log'):
            require(not plain(self.root / name, False).exists(), 'fetch stale output or consumed root')
        self.used = True
        with (self.root / 'fetch.request.json').open('xb') as stream: stream.write(raw)
        process = subprocess.Popen([str(self.host_directory / 'FetchHost.exe'), str(self.root)],
            cwd=str(self.host_directory), env=dict(self.environment), stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        monitoring = monitor_host(process, self.root, OUTER_SECONDS)
        self.identity(); reject_unreviewed_config(self.root)
        inspect_cache_budget(self.root, shutil.disk_usage(self.root).free)
        summary = summarize_fetch_receipt(plan, self.root, self.control_sha, self.run_id)
        summary['resource_monitoring'] = monitoring
        return summary
