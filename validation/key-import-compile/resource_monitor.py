"""Observed disk-reserve/cache monitoring; not OS quotas or egress enforcement."""
import shutil
import subprocess
import time
from adapter.compile_adapter import require
from acquisition_inputs import inspect_cache_budget
POLL_SECONDS = 1
CACHE_SCAN_SECONDS = 5

def monitor_host(process, root, outer_seconds):
    """Wait within actual wall budget; stop owned Host on any monitoring failure.

    Host owns a kill-on-close Job Object. Outer termination is still classified
    unknown and must never be substituted for its successful empty-tree receipt.
    """
    started = time.monotonic(); next_scan = started; samples = 0; scans = 0
    def checkpoint():
        nonlocal samples
        require(time.monotonic() - started < outer_seconds, 'fetch outer deadline')
        from acquisition_inputs import MIN_FREE_BYTES
        require(shutil.disk_usage(root).free >= MIN_FREE_BYTES, 'fetch observed disk reserve')
        samples += 1
    try:
        while True:
            now = time.monotonic()
            require(now - started < outer_seconds, 'fetch outer deadline')
            free = shutil.disk_usage(root).free
            # Keep the same reserve as the initial/final cache inspection.
            from acquisition_inputs import MIN_FREE_BYTES
            require(free >= MIN_FREE_BYTES, 'fetch observed disk reserve')
            samples += 1
            if now >= next_scan:
                inspect_cache_budget(root, free, checkpoint=checkpoint); scans += 1
                next_scan = time.monotonic() + CACHE_SCAN_SECONDS
            require(time.monotonic() - started < outer_seconds, 'fetch outer deadline')
            status = process.poll()
            if status is not None:
                require(status == 0, 'fetch Host failed')
                return dict(disk_samples=samples, cache_scans=scans,
                            elapsed_seconds=time.monotonic() - started,
                            continuous_disk_enforcement=False, os_egress_enforcement=False)
            time.sleep(POLL_SECONDS)
    except BaseException:
        # Only this invocation's process handle; never names/PIDs from a receipt.
        # Regardless of kill outcome, caller must abort the stage, never retry.
        if process.poll() is None:
            try:
                process.kill()
                process.wait(timeout=5)
            except (OSError, subprocess.TimeoutExpired):
                pass
        raise
