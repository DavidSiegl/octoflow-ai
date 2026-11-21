# The gate's golden reports

The `golden/` files of the octoflow-gate 0.15.3 release, the one `ci/gate.pin` names
(`octoflow-gate-0.15.3.tar.gz`, sha256 `bcff7de3e4e699a97ed149db8c321732d199ec99c17766d132517fc2dafed120`), copied
as they are: each case's quality file (`.txt`) and its quality.json (`.json`). engine/src/quality/tests.rs holds the
engine's readers of both to them. `cargo xtask raise-gate-pin` replaces them, and this file's version and sha256, with
the pin.
