#!/usr/bin/env python3
"""Check attribution in the history of a XenTerm Next product commit."""
import argparse
import re
import subprocess
import sys

BOT_NAME = re.compile(r"codex(?:\[bot\])?", re.IGNORECASE)
BOT_EMAIL = re.compile(
    r"codex@openai\.com|(?:\d+\+)?codex(?:\[bot\])?@users\.noreply\.github\.com",
    re.IGNORECASE,
)
COAUTHOR = re.compile(r"\s*Co-authored-by:\s*(.*?)\s*<([^<>]+)>\s*", re.IGNORECASE)


def bot_identity(name, email):
    return bool(BOT_NAME.fullmatch(name.strip()) or BOT_EMAIL.fullmatch(email.strip()))


def violations(revision):
    output = subprocess.check_output([
        "git", "log", "-z", "--encoding=UTF-8",
        "--format=%H%x00%an%x00%ae%x00%cn%x00%ce%x00%B", revision, "--",
    ]).decode("utf-8", errors="replace")
    fields = output.removesuffix("\0").split("\0")
    if not output or len(fields) % 6:
        raise ValueError("Could not read complete Git commit attribution")
    for i in range(0, len(fields), 6):
        sha, author, author_email, committer, committer_email, message = fields[i:i + 6]
        roles = []
        if bot_identity(author, author_email):
            roles.append("author")
        if bot_identity(committer, committer_email):
            roles.append("committer")
        if any(bot_identity(*match.groups()) for line in message.splitlines()
               if (match := COAUTHOR.fullmatch(line))):
            roles.append("co-author")
        if roles:
            yield sha, ", ".join(roles)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("revision", nargs="?", default="HEAD")
    args = parser.parse_args()
    revision = subprocess.check_output([
        "git", "rev-parse", "--verify", "--end-of-options", args.revision + "^{commit}",
    ], text=True).strip()
    if subprocess.check_output(["git", "rev-parse", "--is-shallow-repository"], text=True).strip() != "false":
        parser.error("Fetch full Git history before checking attribution")
    found = list(violations(revision))
    for sha, roles in found:
        print(f"{sha}: Codex bot attribution in {roles}", file=sys.stderr)
    if found:
        return 1
    print(f"Commit attribution verified for {revision}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
