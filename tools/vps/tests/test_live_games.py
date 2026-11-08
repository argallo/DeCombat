import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import live_games as board


class BoardTest(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.state = Path(tmp.name) / "state.json"
        self.source = Path(tmp.name) / "lobby.json"
        self.sources = [("Lobby", self.source)]
        self.hook = "https://discord.com/api/webhooks/123/example"

    def test_display_privacy_limit_empty_and_stale(self):
        game = dict(server="West", channel=2, room=1, map="Synthetic Yard", mode="Gun Game",
                    players=3, bots=1, capacity=8, nick="SECRET", host="SECRET")
        self.source.write_text(json.dumps(dict(version=1, at=100, games=[game])))
        message = board.render(self.sources, 101)
        self.assertIn("West · Channel 2 · Room 001", message)
        self.assertIn("4/8 slots (1 bot)", message)
        self.assertNotIn("SECRET", message)
        self.assertIn("Status unavailable", board.render(self.sources, 200))
        self.source.write_text(json.dumps(dict(version=1, at=100, games=[])))
        self.assertIn("No public matches running", board.render(self.sources, 101))
        self.source.write_text(json.dumps(dict(version=1, at=100, games=[game] * 150)))
        message = board.render(self.sources, 101)
        self.assertLessEqual(len(message.encode("utf-16-le")) // 2, 1900)
        self.assertIn("More matches", message)

    def test_first_post_then_edit_skip_unchanged_and_recover_deleted_message(self):
        calls = []
        ids = iter(("71", "72"))

        def request(method, path, payload):
            self.assertEqual(payload["allowed_mentions"], {"parse": []})
            calls.append((method, path))
            if method == "PATCH" and len(calls) == 4:
                raise board.DiscordError(404, 10008, 0)
            return {"id": next(ids)} if method == "POST" else {}

        # 110: the same board inside the tick is skipped; 130: the same board a tick later is re-stamped
        for content, now in [("first", 100), ("first", 110), ("first", 130), ("changed", 140), ("new state", 200)]:
            board.publish(self.hook, self.state, content, now, request)
        self.assertEqual([c[0] for c in calls], ["POST", "PATCH", "PATCH", "PATCH", "POST"])
        self.assertTrue(calls[1][1].endswith("/messages/71"))
        self.assertEqual(json.loads(self.state.read_text())["message"], "72")

    def test_rate_limit_and_ambiguous_post_do_not_duplicate(self):
        calls = []

        def limited(*args):
            calls.append(args[0])
            raise board.DiscordError(429, 0, 3.5)

        board.publish(self.hook, self.state, "one", 100, limited)
        board.publish(self.hook, self.state, "one", 101, limited)
        self.assertEqual(calls, ["POST"])

        def timeout(*_):
            raise TimeoutError()

        with self.assertRaises(TimeoutError):
            board.publish(self.hook, self.state, "one", 110, timeout)
        with self.assertRaisesRegex(ValueError, "unconfirmed"):
            board.publish(self.hook, self.state, "one", 111, timeout)


if __name__ == "__main__":
    unittest.main()
