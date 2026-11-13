# The gate's golden reports

The `golden/` files of the octoflow-gate 0.10.0 release, the one `ci/gate.pin` names
(`octoflow-gate-0.10.0.tar.gz`, sha256 `8f6007f046be5fcf516496201a41bc7185a3af53c5e7778a9489a0d10414525a`), copied
as they are: each case's quality file (`.txt`) and its quality.json (`.json`). engine/src/quality/tests.rs holds the
engine's readers of both to them. When the pin is raised to a release whose golden files differ, copy that release's
here with it.
