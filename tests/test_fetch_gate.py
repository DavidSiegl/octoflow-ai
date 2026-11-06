"""ci/fetch-gate.sh, which fetches the release a pin names: what it unpacks, and what it refuses before unpacking.

Offline: a curl stub on PATH serves a tarball the test builds, and records each URL it was asked for.
"""

import hashlib
import io
import os
import subprocess
import tarfile
import tempfile
import unittest

import conftest

# conftest.REPO, not this file's directory: under mutmut the tests run from mutants/, which has no ci/.
SCRIPT = os.path.join(conftest.REPO, "ci", "fetch-gate.sh")

# Copies FAKE_TARBALL to the file after -o and appends the URL, its last argument, to CURL_LOG; exits 22, as curl -f
# does on an HTTP error, when FAKE_TARBALL is unset.
CURL_STUB = """#!/bin/sh
for arg; do last=$arg; done
echo "$last" >> "$CURL_LOG"
[ -n "${FAKE_TARBALL:-}" ] || exit 22
while [ $# -gt 0 ]; do
    if [ "$1" = -o ]; then cp "$FAKE_TARBALL" "$2"; exit 0; fi
    shift
done
exit 2
"""


class TestFetchGate(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        bin_dir = os.path.join(self.tmp.name, "bin")
        os.mkdir(bin_dir)
        with open(os.path.join(bin_dir, "curl"), "w") as f:
            f.write(CURL_STUB)
        os.chmod(os.path.join(bin_dir, "curl"), 0o755)  # noqa: S103
        self.tarball = os.path.join(self.tmp.name, "release.tar.gz")
        buf = io.BytesIO()
        with tarfile.open(fileobj=buf, mode="w:gz") as tar:
            body = b"print('the released gate')\n"
            info = tarfile.TarInfo("gate.py")
            info.size = len(body)
            tar.addfile(info, io.BytesIO(body))
        with open(self.tarball, "wb") as f:
            f.write(buf.getvalue())
        self.sha256 = hashlib.sha256(buf.getvalue()).hexdigest()
        self.log = os.path.join(self.tmp.name, "curl.log")
        self.out = os.path.join(self.tmp.name, "out")
        self.env = {**os.environ, "PATH": f"{bin_dir}:{os.environ['PATH']}", "CURL_LOG": self.log,
                    "FAKE_TARBALL": self.tarball}

    def fetch(self, pin_text):
        """Run fetch-gate.sh on a pin holding pin_text, into self.out; returns (exit status, stdout, stderr)."""
        pin = os.path.join(self.tmp.name, "gate.pin")
        with open(pin, "w") as f:
            f.write(pin_text)
        run = subprocess.run(["sh", SCRIPT, pin, self.out], env=self.env,  # noqa: S603, S607
                             capture_output=True, text=True, check=False)
        return run.returncode, run.stdout, run.stderr

    def urls(self):
        """The URLs curl was asked for, in order."""
        try:
            with open(self.log) as f:
                return f.read().split()
        except FileNotFoundError:
            return []

    def test_a_matching_release_is_unpacked_and_its_version_printed(self):
        status, stdout, _ = self.fetch(f"# a comment\n0.5.0 {self.sha256}\n")
        self.assertEqual((status, stdout), (0, "0.5.0\n"))
        release = "https://github.com/DavidSiegl/octoflow-gate/releases/download/v0.5.0"
        self.assertEqual(self.urls(), [f"{release}/octoflow-gate-0.5.0.tar.gz"])
        with open(os.path.join(self.out, "gate.py")) as f:
            self.assertEqual(f.read(), "print('the released gate')\n")

    def test_a_tarball_that_does_not_match_the_sha256_unpacks_nothing(self):
        status, _, stderr = self.fetch(f"0.5.0 {'0' * 64}\n")
        self.assertEqual(status, 1)
        self.assertIn("does not match the sha256", stderr)
        self.assertFalse(os.path.exists(self.out))

    def test_a_failed_download_fails(self):
        del self.env["FAKE_TARBALL"]
        status, _, stderr = self.fetch(f"0.5.0 {self.sha256}\n")
        self.assertEqual(status, 1)
        self.assertIn("cannot download", stderr)
        self.assertFalse(os.path.exists(self.out))

    def test_a_malformed_pin_is_refused_before_any_download(self):
        for pin_text in ["# only a comment\n", "0.5.0\n", f"0.5.0 {self.sha256[:-1]}\n",
                         f"0.5.0 {self.sha256.upper()}\n", f"../0.5.0 {self.sha256}\n", f"0.5.0?x {self.sha256}\n"]:
            with self.subTest(pin_text=pin_text):
                status, _, stderr = self.fetch(pin_text)
                self.assertEqual(status, 1)
                self.assertIn("is not '<version> <sha256>'", stderr)
        self.assertEqual(self.urls(), [])

    def test_an_unreadable_pin_is_refused(self):
        run = subprocess.run(["sh", SCRIPT, os.path.join(self.tmp.name, "missing.pin"), self.out],  # noqa: S603, S607
                             env=self.env, capture_output=True, text=True, check=False)
        self.assertEqual(run.returncode, 1)
        self.assertIn("cannot read the pin", run.stderr)
        self.assertEqual(self.urls(), [])
