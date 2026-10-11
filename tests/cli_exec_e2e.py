"""CLI exit-status and option-boundary regressions using synthetic loopback SSH."""
import argparse
import json
from pathlib import Path
import subprocess
import tempfile
import threading
import time

from ssh_jump_chain_e2e import Fixture, PASSWORD, Server


def reply_to_command(server, channel, command):
    server.node.commands.append(command.decode())

    def reply():
        try:
            if command.startswith(b"delayed"):
                time.sleep(1.3)
            elif command == b"stall":
                time.sleep(3)
            channel.sendall(b"received:" + command + b"\n")
            if command == b"fail":
                channel.send_stderr(b"synthetic remote failure\n")
                channel.send_exit_status(23)
            elif command != b"missing-status":
                channel.send_exit_status(0)
            channel.close()
        except (EOFError, OSError):
            pass  # Timeout deliberately closes the fixture transport.

    threading.Thread(target=reply, daemon=True).start()
    return True


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--exe", required=True, type=Path)
    args = parser.parse_args()
    Server.check_channel_exec_request = reply_to_command
    with tempfile.TemporaryDirectory(prefix="xenterm-cli-exit-") as directory:
        fixture = Fixture(args.exe.resolve(), directory)
        for node in fixture.nodes:
            node.commands = []
        try:
            def run(options, command, ok):
                result = subprocess.run([str(fixture.exe), "--data-dir", str(fixture.config),
                    "cli", "exec", "target", *options, "--", *command],
                    capture_output=True, text=True, timeout=15, env=fixture.env)
                assert (result.returncode == 0) == ok, (result.returncode, result.stdout, result.stderr)
                assert PASSWORD not in result.stdout + result.stderr
                return result

            good = run(["--json"], ["success"], True)
            value = json.loads(good.stdout)
            assert value["exit_code"] == 0 and value["stdout"] == "received:success\n"
            bad = run(["--json"], ["fail"], False)
            value = json.loads(bad.stdout)
            assert value["exit_code"] == 23 and value["stderr"] == "synthetic remote failure\n"
            assert "remote command failed" in bad.stderr
            human = run([], ["fail"], False)
            assert human.stdout == "received:fail\n"
            assert "synthetic remote failure" in human.stderr and "remote command failed" in human.stderr
            missing = run(["--json"], ["missing-status"], False)
            assert json.loads(missing.stdout)["exit_code"] is None
            timed = run(["--json", "--timeout", "1"], ["stall"], False)
            assert json.loads(timed.stdout)["timed_out"] is True
            assert "remote command timed out" in timed.stderr
            print("PASS: two-hop CLI returns failure for nonzero/missing exit status and timeout, retaining JSON output")

            for command in (["capture", "--json", "--timeout", "not-a-number"],
                            ["delayed", "--timeout", "1", "--json"]):
                result = run([], command, True)
                assert result.stdout == "received:" + " ".join(command) + "\n"
                assert fixture.nodes[-1].commands[-1] == " ".join(command)
            result = run(["--json", "--timeout", "5"], ["capture", "--timeout", "not-a-number"], True)
            assert json.loads(result.stdout)["stdout"] == "received:capture --timeout not-a-number\n"
            print("PASS: --json/--timeout after -- reach the remote command unchanged and do not change local output/deadlines")
        finally:
            for node in fixture.nodes:
                node.close()


if __name__ == "__main__":
    main()
