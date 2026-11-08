#!/usr/bin/env python3
"""Publish a read-only DeCombat match board from local, public-only snapshots."""

import argparse
import hashlib
import http.client
import json
import os
import re
import sys
import tempfile
import time
from pathlib import Path
from urllib.parse import urlsplit

try:
    import fcntl
except ImportError:  # Windows only runs the unit tests; the publisher runs on Linux
    fcntl = None


def label(value):
    if not isinstance(value, str):
        raise ValueError("expected a label")
    # Server and mod labels can contain formatting, mentions or links.
    clean = "".join(c if c.isalnum() or c in " -'+()" else " " for c in value)
    return " ".join(clean.split())[:56] or "Unknown"


def number(value, minimum=0):
    if type(value) is not int or value < minimum:
        raise ValueError("invalid count")
    return value


def game_line(game):
    channel = number(game["channel"], 1)
    room = number(game["room"], 1)
    humans, bots = number(game["players"]), number(game["bots"])
    capacity = number(game["capacity"], 1)
    seats = f"{humans + bots}/{capacity} slots"
    if bots:
        seats += f" ({bots} bot{'s' if bots != 1 else ''})"
    return (f"**{label(game['server'])} · Channel {channel} · Room {room:03}**\n"
            f"{label(game['map'])} · {label(game['mode'])} · {seats}")


def lobby_lines(name, path, now):
    try:
        with path.open("rb") as stream:
            data = json.loads(stream.read(1_000_001))
        if data["version"] != 1 or not 0 <= now - number(data["at"]) <= 90:
            raise ValueError("stale or incompatible snapshot")
        if not isinstance(data["games"], list):
            raise ValueError("invalid games list")
        games = [game_line(game) for game in data["games"]]
        return [f"**{name}**", *(games or ["No public matches running."])]
    except (OSError, ValueError, KeyError, TypeError):
        return [f"**{name}**", "Status unavailable."]


def render(sources, now):
    lines = [line for name, path in sources for line in lobby_lines(name, path, now)]
    board = "**DeCombat · Live games**"
    for line in lines:
        candidate = board + "\n" + line
        # Discord limits content to 2000 characters. Leave room for the clock
        # and truncation line, and count astral characters as two to be safe.
        if len(candidate.encode("utf-16-le")) // 2 > 1890:
            board += "\nMore matches are listed in-game."
            break
        board = candidate
    return board


class DiscordError(Exception):
    def __init__(self, status, code, retry_after):
        super().__init__(f"Discord returned HTTP {status} (code {code})")
        self.status, self.code, self.retry_after = status, code, retry_after


def discord_request(method, path, body):
    conn = http.client.HTTPSConnection("discord.com", timeout=12)
    try:
        conn.request(method, path, json.dumps(body), {
            "Content-Type": "application/json", "User-Agent": "DeCombat live-games board/1"
        })
        res = conn.getresponse()
        data = json.loads(res.read(100_000))
        if res.status != 200:
            raise DiscordError(res.status, data.get("code"), data.get("retry_after", 60))
        return data
    finally:
        conn.close()


def save_state(path, state):
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as tmp:
        tmp.write(json.dumps(state).encode())
        tmp.flush()
        os.fsync(tmp.fileno())
    os.replace(tmp.name, path)


def publish(webhook, state_file, board, now, request=discord_request):
    url = urlsplit(webhook)
    match = re.fullmatch(r"/api/(?:v[0-9]+/)?webhooks/([0-9]+)/[A-Za-z0-9._-]+", url.path)
    if url.scheme != "https" or url.netloc != "discord.com" or not match or url.query or url.fragment:
        raise ValueError("expected an HTTPS discord.com webhook URL")
    state = json.loads(state_file.read_text()) if state_file.exists() else {}
    hook_id = match.group(1)
    if state and state.get("webhook") != hook_id:
        raise ValueError("state belongs to a different webhook")
    if state.get("pending"):
        raise ValueError("initial post unconfirmed; reconcile its message ID before retrying")
    if now < state.get("retry_at", 0):
        return
    digest = hashlib.sha256(board.encode()).hexdigest()
    # An unchanged board is re-stamped on every timer tick (30 s), so "Last checked" shows a stalled publisher
    # within a minute; the guard only swallows a second run inside the same tick.
    if digest == state.get("digest") and now - state.get("updated", 0) < 20:
        return
    payload = {"content": board + f"\nLast checked <t:{now}:R>.", "allowed_mentions": {"parse": []}}
    message = state.get("message")
    try:
        if message:
            try:
                request("PATCH", f"{url.path}/messages/{message}", payload)
            except DiscordError as exc:
                if (exc.status, exc.code) != (404, 10008):
                    raise
                message = None
        if not message:
            # A timeout after POST is ambiguous. Mark it before sending so a
            # reboot or timeout cannot silently create a second board.
            save_state(state_file, {"webhook": hook_id, "pending": True})
            response = request("POST", url.path + "?wait=true", payload)
            message = response["id"]
            if not isinstance(message, str) or not message.isdecimal():
                raise ValueError("Discord did not return a message ID")
    except DiscordError as exc:
        if exc.status == 429:
            save_state(state_file, {"webhook": hook_id, "message": message,
                                    "retry_at": now + max(1, int(float(exc.retry_after)) + 1)})
            return
        if not message and 400 <= exc.status < 500:
            save_state(state_file, {"webhook": hook_id})
        raise
    save_state(state_file, {"webhook": hook_id, "message": message, "digest": digest, "updated": now})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", action="append", required=True, metavar="LABEL=/path/to/snapshot.json")
    parser.add_argument("--print", action="store_true", dest="preview")
    parser.add_argument("--state", type=Path)
    parser.add_argument("--webhook-file", type=Path)
    args = parser.parse_args()
    if not args.preview and not (args.state and args.webhook_file):
        parser.error("publishing requires --state and --webhook-file")
    sources = []
    for source in args.source:
        name, sep, path = source.partition("=")
        if not sep or not name or not Path(path).is_absolute():
            parser.error("--source requires LABEL=/absolute/path")
        sources.append((label(name), Path(path)))
    if args.preview:
        print(render(sources, int(time.time())))
        return
    with args.state.with_suffix(".lock").open("a") as lock:
        if fcntl is not None:
            fcntl.flock(lock, fcntl.LOCK_EX)
        now = int(time.time())
        publish(args.webhook_file.read_text().strip(), args.state, render(sources, now), now)


if __name__ == "__main__":
    try:
        main()
    except DiscordError as exc:
        sys.exit(str(exc))
    except (OSError, http.client.HTTPException, KeyError, ValueError, TypeError):
        # Never log a response body, URL or credential while handling an error.
        sys.exit("Live games update failed; check the local state, snapshot and publisher configuration.")
