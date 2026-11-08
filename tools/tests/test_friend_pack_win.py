"""Unit tests of tools/friend_pack_win.py's build-stamp check (CLI-1944) and a --help smoke test.

    python3 -I -m unittest discover -s tools/tests -p "test_*.py"
"""
import importlib.util
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
TOOL = os.path.join(os.path.dirname(HERE), "friend_pack_win.py")


def load_tool():
    spec = importlib.util.spec_from_file_location("friend_pack_win", TOOL)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


fp = load_tool()

STAMP = b"DCSTAMP1;commit=1ad4f338;version=dev;target=x86_64-pc-windows-msvc;profile=release;end"


def fake_exe(stamp=STAMP) -> bytes:
    """A blob shaped like a PE file around the stamp: junk, the stamp, more junk."""
    junk = bytes(range(256)) * 40
    return b"MZ" + junk + b"\x00version dev; commit unknown\x00" + stamp + b"\x00" + junk[::-1]


class StampFinder(unittest.TestCase):
    def test_finds_the_stamp_between_junk(self):
        s = fp.find_stamp(fake_exe())
        self.assertEqual(s, {"commit": "1ad4f338", "version": "dev", "target": "x86_64-pc-windows-msvc",
                             "profile": "release"})

    def test_no_stamp_is_none(self):
        self.assertIsNone(fp.find_stamp(fake_exe(b"")))
        self.assertIsNone(fp.find_stamp(b"DCSTAMP1;commit=abc;version=dev;end"), "a cut stamp is no stamp")

    def test_a_tagged_release_stamp(self):
        s = fp.find_stamp(fake_exe(b"DCSTAMP1;commit=44c51cbb;version=0.3.0;target=x86_64-pc-windows-msvc;"
                                   b"profile=release;end"))
        self.assertEqual((s["commit"], s["version"]), ("44c51cbb", "0.3.0"))

    def test_same_commit_is_a_prefix_match_of_seven_digits_or_more(self):
        self.assertTrue(fp.same_commit("1ad4f338", "1ad4f338abcdef0123456789abcdef0123456789"))
        self.assertTrue(fp.same_commit("1AD4F338", "1ad4f338"))
        self.assertFalse(fp.same_commit("1ad4f338", "1ad4f339"))
        self.assertFalse(fp.same_commit("1ad4f", "1ad4f338"), "too short to mean anything")


class ExeCheck(unittest.TestCase):
    def test_the_matching_build_passes(self):
        self.assertIsNone(fp.check_exe("x.exe", "1ad4f338", blob=fake_exe()))
        self.assertIsNone(fp.check_exe("x.exe", "1ad4f338abcdef01", blob=fake_exe()))

    def test_a_stale_exe_under_another_name_is_refused(self):
        why = fp.check_exe("x.exe", "25215efa", blob=fake_exe())
        self.assertIn("is build 1ad4f338", why)
        self.assertIn("--commit says 25215efa", why)

    def test_a_missing_commit_is_refused(self):
        self.assertIn("--commit is required", fp.check_exe("x.exe", "", blob=fake_exe()))

    def test_an_unstamped_or_dev_exe_is_refused_unless_allowed(self):
        self.assertIn("no build stamp", fp.check_exe("x.exe", "1ad4f338", blob=fake_exe(b"")))
        dev = fake_exe(b"DCSTAMP1;commit=unknown;version=dev;target=x86_64-pc-windows-msvc;profile=debug;end")
        self.assertIn("dev build", fp.check_exe("x.exe", "1ad4f338", blob=dev))
        self.assertIsNone(fp.check_exe("x.exe", "1ad4f338", allow_unstamped=True, blob=fake_exe(b"")))
        self.assertIsNone(fp.check_exe("x.exe", "1ad4f338", allow_unstamped=True, blob=dev))

    def test_built_after_refuses_an_older_file(self):
        self.assertIn("before --built-after", fp.check_exe("x.exe", "1ad4f338", built_after=2000.0, blob=fake_exe(),
                                                            mtime=1000.0))
        self.assertIsNone(fp.check_exe("x.exe", "1ad4f338", built_after=1000.0, blob=fake_exe(), mtime=2000.0))

    def test_the_tool_refuses_before_writing_anything(self):
        with tempfile.TemporaryDirectory() as d:
            exe = os.path.join(d, "dc-client.exe")
            with open(exe, "wb") as f:
                f.write(fake_exe())
            out = os.path.join(d, "out")
            r = subprocess.run([sys.executable, "-I", TOOL, "--client", exe, "--server", "example.test",
                                "--commit", "25215efa", "--out", out], capture_output=True, text=True)
            self.assertEqual(r.returncode, 2, r.stderr)
            self.assertIn("refused:", r.stderr)
            self.assertFalse(os.path.exists(out), "nothing written")
            r = subprocess.run([sys.executable, "-I", TOOL, "--client", exe, "--server", "example.test",
                                "--commit", "1ad4f338", "--out", out], capture_output=True, text=True)
            self.assertEqual(r.returncode, 0, r.stderr)
            self.assertTrue(os.path.exists(os.path.join(out, "DeCombat-win-1ad4f338.zip")))


class HelpSmoke(unittest.TestCase):
    def test_help_exits_zero_and_names_the_check(self):
        r = subprocess.run([sys.executable, "-I", TOOL, "--help"], capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("--built-after", r.stdout)
        self.assertIn("--allow-unstamped", r.stdout)


if __name__ == "__main__":
    unittest.main()
