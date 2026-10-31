"""Pure monitored-launch logic tests; fake process only, never starts Cargo/Host."""
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch
import owned_fetch as f
import resource_monitor as monitor
from acquisition_inputs import MIN_FREE_BYTES


class Monitor(unittest.TestCase):
    def setUp(self):
        self.process = Mock(); self.process.poll.return_value = 0
        self.disk = patch.object(monitor.shutil, 'disk_usage', return_value=SimpleNamespace(free=MIN_FREE_BYTES + 1))
        self.scan = patch.object(monitor, 'inspect_cache_budget', return_value={})
        self.sleep = patch.object(monitor.time, 'sleep')
        self.disk.start(); self.scan.start(); self.sleep.start()
        self.addCleanup(patch.stopall)

    def test_natural_return_reports_sampling_not_quota(self):
        result = monitor.monitor_host(self.process, Path('/synthetic'), f.OUTER_SECONDS)
        self.assertEqual(result['disk_samples'], 1); self.assertEqual(result['cache_scans'], 1)
        self.assertIs(result['continuous_disk_enforcement'], False)
        self.assertIs(result['os_egress_enforcement'], False)
        self.process.kill.assert_not_called()

    def test_observed_low_disk_terminates_only_owned_handle(self):
        self.process.poll.return_value = None
        monitor.shutil.disk_usage.return_value.free = MIN_FREE_BYTES - 1
        with self.assertRaisesRegex(ValueError, 'disk reserve'): monitor.monitor_host(self.process, Path('/synthetic'), f.OUTER_SECONDS)
        self.process.kill.assert_called_once_with(); self.process.wait.assert_called_once_with(timeout=5)

    def test_cache_violation_stops_and_never_success(self):
        self.process.poll.return_value = None
        monitor.inspect_cache_budget.side_effect = ValueError('cache budget')
        with self.assertRaisesRegex(ValueError, 'cache budget'): monitor.monitor_host(self.process, Path('/synthetic'), f.OUTER_SECONDS)
        self.process.kill.assert_called_once()

    def test_outer_clock_expiry_stops(self):
        self.process.poll.return_value = None
        with patch.object(monitor.time, 'monotonic', side_effect=[0, f.OUTER_SECONDS]):
            with self.assertRaisesRegex(ValueError, 'deadline'): monitor.monitor_host(self.process, Path('/synthetic'), f.OUTER_SECONDS)
        self.process.kill.assert_called_once()

    def test_scan_elapsed_time_is_in_budget(self):
        self.process.poll.return_value = None
        with patch.object(monitor.time, 'monotonic', side_effect=[0, 1, 616, 616]):
            with self.assertRaisesRegex(ValueError, 'deadline'): monitor.monitor_host(self.process, Path('/synthetic'), f.OUTER_SECONDS)
        self.process.kill.assert_called_once()

    def test_nonzero_host_is_not_qualified(self):
        self.process.poll.return_value = 1
        with self.assertRaisesRegex(ValueError, 'Host failed'): monitor.monitor_host(self.process, Path('/synthetic'), f.OUTER_SECONDS)
        self.process.kill.assert_not_called()

    def test_cleanup_uncertainty_never_turns_failure_into_success(self):
        self.process.poll.return_value = None
        monitor.shutil.disk_usage.side_effect = OSError('unavailable')
        self.process.wait.side_effect = subprocess.TimeoutExpired('owned Host', 5)
        with self.assertRaises(OSError): monitor.monitor_host(self.process, Path('/synthetic'), f.OUTER_SECONDS)
        self.process.kill.assert_called_once()

    def test_free_space_checked_on_every_poll(self):
        self.process.poll.side_effect = [None, 0]
        with patch.object(monitor.time, 'monotonic', side_effect=[0, 0, 0, 0, 1, 1, 1]):
            result = monitor.monitor_host(self.process, Path('/synthetic'), f.OUTER_SECONDS)
        self.assertEqual(result['disk_samples'], 2)
        self.assertEqual(result['cache_scans'], 1)
        monitor.time.sleep.assert_called_once_with(1)

    def test_deadline_during_incremental_scan_stops_owned_host(self):
        self.process.poll.return_value = None
        monitor.inspect_cache_budget.side_effect = lambda root, free, checkpoint: checkpoint()
        with patch.object(monitor.time, 'monotonic', side_effect=[0, 1, 616]):
            with self.assertRaisesRegex(ValueError, 'deadline'):
                monitor.monitor_host(self.process, Path('/synthetic'), f.OUTER_SECONDS)
        self.process.kill.assert_called_once()

    def test_disk_drop_during_incremental_scan_stops_owned_host(self):
        self.process.poll.return_value = None
        monitor.inspect_cache_budget.side_effect = lambda root, free, checkpoint: checkpoint()
        monitor.shutil.disk_usage.side_effect = [SimpleNamespace(free=MIN_FREE_BYTES+1), SimpleNamespace(free=MIN_FREE_BYTES-1)]
        with self.assertRaisesRegex(ValueError, 'disk reserve'):
            monitor.monitor_host(self.process, Path('/synthetic'), f.OUTER_SECONDS)
        self.process.kill.assert_called_once()


class Launcher(unittest.TestCase):
    def test_single_use_rejects_without_process(self):
        launcher = object.__new__(f.OwnedFetch); launcher.used = True
        with patch.object(f.subprocess, 'Popen') as spawn, self.assertRaisesRegex(ValueError, 'consumed'):
            launcher({})
        spawn.assert_not_called()

    def test_identity_failure_cannot_spawn_or_mark_used(self):
        launcher = object.__new__(f.OwnedFetch); launcher.used = False
        launcher.identity = Mock(side_effect=ValueError('pin mismatch'))
        with patch.object(f.subprocess, 'Popen') as spawn, self.assertRaisesRegex(ValueError, 'pin mismatch'):
            launcher({})
        spawn.assert_not_called(); self.assertFalse(launcher.used)

    def test_launch_has_no_shell_inherited_environment_or_raw_log_upload(self):
        source = Path(f.__file__).read_text()
        self.assertIn('env=dict(self.environment)', source)
        self.assertNotIn('shell=True', source); self.assertNotIn('os.environ.copy', source)
        self.assertIn('stdout=subprocess.DEVNULL', source)
        self.assertIn('self.used = True', source)
        self.assertIn("open('xb')", source)
        self.assertIn('self.transport_gate(); self.host_entry_gate(); self.identity_gate()', source)
        self.assertIn("record.get('host_sources') == sources", source)


if __name__ == '__main__': unittest.main()
