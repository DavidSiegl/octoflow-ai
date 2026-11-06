"""Rules the docs state and no example test would notice breaking, one test each.

Each test's failure message names the document that states its rule.
"""

import time
import tomllib
import unittest

import conftest
from guard_errors import RealSleep

ROOT = conftest.REPO


class TestInvariants(unittest.TestCase):
    def test_the_engine_s_version_is_the_release_s(self):
        with open(ROOT / "pyproject.toml", "rb") as f:
            release = tomllib.load(f)["project"]["version"]
        with open(ROOT / "engine" / "Cargo.toml", "rb") as f:
            engine = tomllib.load(f)["package"]["version"]
        self.assertEqual(engine, release, "the engine's version is the release's, which the Jenkinsfile's Release "
                                          "stage reads from pyproject.toml (docs/architecture.md, Tooling)")

    def test_no_test_sleeps_for_real(self):
        with self.assertRaises(RealSleep, msg="a test that must wait on a real process is marked real_sleep "
                                              "(tests/conftest.py)"):
            time.sleep(0)
