"""Bound command lifetime and combined output without hiding a nonzero exit."""
import os
import pathlib
import selectors
import signal
import subprocess
import sys
import time

seconds, output, *command = sys.argv[1:]
deadline = time.monotonic() + int(seconds)
limit = 16 * 1024**2
size = 0
process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                           start_new_session=True)
selector = selectors.DefaultSelector()
selector.register(process.stdout, selectors.EVENT_READ)
try:
    with pathlib.Path(output).open("xb") as log:
        while selector.get_map():
            if time.monotonic() >= deadline:
                raise RuntimeError("Command timeout")
            for key, _ in selector.select(timeout=0.2):
                chunk = os.read(key.fileobj.fileno(), 65536)
                if not chunk:
                    selector.unregister(key.fileobj)
                    continue
                size += len(chunk)
                if size > limit:
                    raise RuntimeError("Command output exceeded 16 MiB")
                log.write(chunk)
        code = process.wait(timeout=max(0.1, deadline - time.monotonic()))
        if code:
            raise RuntimeError(f"Command failed with exit {code}; see {output}")
finally:
    # Descendants must not survive even if the original process exits first.
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait()
    selector.close()
print(f"Completed {command[0]}: {size} output bytes in {output}")
