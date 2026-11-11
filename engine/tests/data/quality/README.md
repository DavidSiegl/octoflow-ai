# The gate's golden reports

The `golden/` files of the octoflow-gate 0.7.0 release, the one `ci/gate.pin` names
(`octoflow-gate-0.7.0.tar.gz`, sha256 `9700f6072b5d6926b9ce32c01ed582fde7bf887424b62e9ec0ced868b24509e1`), copied
as they are: each case's quality file (`.txt`) and its quality.json (`.json`). engine/src/quality/tests.rs holds the
engine's readers of both to them. When the pin is raised to a release whose golden files differ, copy that release's
here with it.
