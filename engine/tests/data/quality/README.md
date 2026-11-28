# The gate's golden reports

The `golden/` files of the octoflow-gate 0.16.4 release, the one `ci/gate.pin` names
(`octoflow-gate-0.16.4.tar.gz`, sha256 `f19ddd478d5274e81e0d18057bfbb97f7ee1ad268f261102a78455c40849a49c`), copied
as they are: each case's quality file (`.txt`) and its quality.json (`.json`). engine/src/quality/tests.rs holds the
engine's readers of both to them. `cargo xtask raise-gate-pin` replaces them, and this file's version and sha256, with
the pin.
