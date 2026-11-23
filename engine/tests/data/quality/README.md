# The gate's golden reports

The `golden/` files of the octoflow-gate 0.16.0 release, the one `ci/gate.pin` names
(`octoflow-gate-0.16.0.tar.gz`, sha256 `3ae7bc4d6d779c4cee61106ca2c4b70ffebfbd72922e9ecd1ed8419e8cd5d996`), copied
as they are: each case's quality file (`.txt`) and its quality.json (`.json`). engine/src/quality/tests.rs holds the
engine's readers of both to them. `cargo xtask raise-gate-pin` replaces them, and this file's version and sha256, with
the pin.
