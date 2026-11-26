# Octoflow AI

The orchestrator drives a coding task through a workflow of agent turns, from spec to pull request, and keeps each
run's record in the project so it can be shown, resumed or closed.

## Language

**Harness**:
The coding agent's CLI a role's agent runs in, one of `HARNESS_NAMES` (claude, codex, opencode, pi, agy); herdr starts
it by that name, its `--kind`. A role's next agent may run in another harness than its current one, after a resume's
`--role-agent`. The saved keys `agent_kinds` and an agent's `kind`, and the flag's `ROLE=KIND`, keep the older word.
_Avoid_: agent kind, kind (outside herdr's flag and those saved keys)

**Saved run**:
A run's `state.json` as read back: its age, and either its `RunState` or why it cannot be read (not JSON naming a run,
or of a format this engine does not read).
_Avoid_: run state (for the file read back), raw state

**Health**:
Whether an orchestrator still drives a run, as its heartbeat and owner record tell, judged from this machine: running,
stale (its orchestrator died or its machine was lost before it released the run), or none for a run nothing drives —
one finished, failed or released. Separate from whether the run is closed or its state reads: `list` shows a stale run
it cannot read as stale.
_Avoid_: status, liveness
