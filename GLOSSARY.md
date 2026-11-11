# Octoflow AI

The orchestrator drives a coding task through a workflow of agent turns, from spec to pull request, and keeps each
run's record in the project so it can be shown, resumed or closed.

## Language

**Saved run**:
A run's `state.json` as read back: its age, and either its `RunState` or why its format cannot be read.
_Avoid_: run state (for the file read back), raw state
