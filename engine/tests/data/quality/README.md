# The gate's golden reports

The `golden/` files of the octoflow-gate 0.15.2 release, the one `ci/gate.pin` names
(`octoflow-gate-0.15.2.tar.gz`, sha256 `de6173ed3264d1ea635f03d4240c5de1b02f0c5122e37e8a39b5ce5010d4e106`), copied
as they are: each case's quality file (`.txt`) and its quality.json (`.json`). engine/src/quality/tests.rs holds the
engine's readers of both to them. `cargo xtask raise-gate-pin` replaces them, and this file's version and sha256, with
the pin.
