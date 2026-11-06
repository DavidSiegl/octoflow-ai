# The gate's golden reports

The `golden/` files of the octoflow-gate 0.5.0 release, the one `ci/gate.pin` names
(`octoflow-gate-0.5.0.tar.gz`, sha256 `4ebf96264d5ae20cc2dd0516122cd50ebf11dac9f1944c14d899aece882773f8`), copied
as they are: each case's quality file (`.txt`) and its quality.json (`.json`). engine/src/quality/tests.rs holds the
engine's readers of both to them. When the pin is raised to a release whose golden files differ, copy that release's
here with it.
