"""Ordinary CLI/MCP SSH operations on the Windows-sized 1 MiB main stack.

Run: python tests/ssh_stack_e2e.py --exe /path/to/xenterm
Windows uses its native default stack; POSIX subprocesses inherit a 1 MiB
soft stack limit. Only generated credentials, loopback SSH and temporary
profiles are used. Requires the same dependencies as ssh_jump_chain_e2e.py.
"""
import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import subprocess
import time

from ssh_jump_chain_e2e import Fixture, fixture_directory


@contextmanager
def windows_sized_child_stacks():
    if os.name == "nt":
        yield
        return
    import resource
    previous = resource.getrlimit(resource.RLIMIT_STACK)
    # Set this before starting fixture threads. Avoid preexec_fn, which is not
    # safe in a multithreaded Python process. The hard limit remains unchanged.
    resource.setrlimit(resource.RLIMIT_STACK, (1024 * 1024, previous[1]))
    try:
        yield
    finally:
        resource.setrlimit(resource.RLIMIT_STACK, previous)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--exe", required=True, type=Path)
    args = parser.parse_args()
    with windows_sized_child_stacks(), fixture_directory() as root:
        fixture = Fixture(args.exe.resolve(), root)
        try:
            for name in ("outer", "inner", "target"):
                assert fixture.command(name)["stdout"] == f"multi-hop-command:{name}\n"
            print("PASS: direct, one-hop and two-hop MCP commands on a 1 MiB stack")

            fixture.nodes[1].interactive = True
            fixture.command()
            fixture.nodes[1].interactive = False
            fixture.use_key(1, inline=False, encrypted=True)
            fixture.use_key(2, inline=True, encrypted=True)
            fixture.command()
            result = subprocess.run(
                [str(fixture.exe), "--data-dir", str(fixture.config), "cli", "exec",
                 "target", "--json", "--", "fixture"],
                text=True, encoding="utf-8", capture_output=True, timeout=30, env=fixture.env)
            assert result.returncode == 0, result.stderr
            assert json.loads(result.stdout)["stdout"] == "multi-hop-command:target\n"
            print("PASS: keyboard-interactive fallback and encrypted key commands in MCP/CLI")

            for tool, path in (("list_remote_files", "."), ("read_remote_text_file", "/fixture.txt")):
                result = fixture.mcp(tool, session_id="target", path=path, timeout_seconds=8)
                assert not result.get("isError"), result
                assert "fixture" in json.dumps(result["structuredContent"]), result
            print("PASS: two-hop MCP SFTP listing and reading on a 1 MiB stack")

            fixture.trust(omit="outer")
            result = fixture.mcp("run_command", session_id="target", command="fixture", timeout_seconds=8)
            assert result.get("isError"), "unknown host key must fail closed"
            fixture.trust()
            fixture.nodes[1].stall_auth = True
            started = time.monotonic()
            result = fixture.mcp("run_command", session_id="target", command="fixture", timeout_seconds=1)
            elapsed = time.monotonic() - started
            assert not result.get("isError"), result
            assert result["structuredContent"]["timed_out"], result
            assert elapsed < 10, f"1-second route timeout took {elapsed:.1f} seconds"
            print("PASS: host-key rejection and route timeout remain bounded")
        finally:
            for node in fixture.nodes:
                node.close()


if __name__ == "__main__":
    main()
