#!/usr/bin/env python3
"""deploy_hosts.py: reads the lobby's GET /status on stdin, prints which build each game host agent reports and how many
games it runs (tools/vps/deploy_remote.sh hosts). argv[1] is the desired commit's short id.
Exit 0: every configured host is online on it. 1: some are not. 2: the lobby did not answer."""
import json
import sys


def main() -> int:
    short = sys.argv[1]
    try:
        hosts = json.load(sys.stdin).get("hosts", [])
    except ValueError:
        print("the lobby did not answer")
        return 2
    pending = 0
    for h in hosts:
        version = h.get("version", "") if h.get("online") else ""
        same = bool(version) and (version.startswith(short) or short.startswith(version))
        pending += 0 if same else 1
        if h.get("online"):
            state = f"on {version} ({'desired' if same else 'not yet'})"
        else:
            state = "offline"
        print(f"host {h.get('name')}: {state}, {h.get('games', 0):.0f} game(s) running")
    if not hosts:
        print("this lobby has no game hosts configured")
    return 1 if pending else 0


if __name__ == "__main__":
    sys.exit(main())
