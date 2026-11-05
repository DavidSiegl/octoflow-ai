# Agent setup

Generated from the agent-estate template; `copier update` rewrites this file, so change the template, or an answer
with `uvx copier update --trust --defaults -d <key>=<value>` (the `agent-estate:init-repo` skill), not the text
here. CLAUDE.md imports it.

## Issue tracker

Specs and tickets live under `.scratch/` (gitignored), mirrored two-way to GitHub Issues on DavidSiegl/octoflow-ai
by ticket-sync. See `docs/agents/issue-tracker.md`.

## Triage labels

Default vocabulary: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See
`docs/agents/triage-labels.md`.

## Domain docs

Single-context: one `GLOSSARY.md` and `docs/adr/` at the repo root, created lazily, unless the repo already records
its decisions elsewhere. See `docs/agents/domain.md`.

## Review gate

On: Claude's pushes of a commit with no `Verdict: pass` record under `.scratch/reviews/` are refused. Run the branch
through the `cross-review` skill before pushing. A fresh clone has the gate off until `.scratch/reviews/` is created
at its root.
