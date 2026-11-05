# Issue tracker: `.scratch/`, mirrored to GitHub Issues

Specs and tickets are markdown files under `.scratch/<feature-slug>/`, which git ignores. The layout, the `Status:`
line and every tracker operation (publish, fetch, claim, resolve, wayfinding) are the `scratch-issue-tracker` skill's;
follow it rather than this file for those.

## Mirrored to GitHub

`git config agent.ticketSync` is `DavidSiegl/octoflow-ai` in this checkout (set when the template is applied), so the
`ticket-sync` skill keeps every open ticket in step with an issue on DavidSiegl/octoflow-ai, both ways:
a `Status:` becomes the issue's triage label, and comments from collaborators come back under `## Comments`.
Consequences worth knowing before writing a ticket:

- **An open ticket is published** to everyone who can read DavidSiegl/octoflow-ai; write nothing in a ticket you would
  not post as an issue.
- **An issue you open on GitHub becomes a ticket** under `.scratch/github/issues/`, numbered with its issue number.
  Issues opened by anyone else are not imported; to act on one, write a ticket for it.
- **A fresh clone has no tickets and no sync** until `git config agent.ticketSync DavidSiegl/octoflow-ai` sets it again;
  the next sync then pulls the open tickets back from their issues.

## Pull requests as a triage surface

**PRs as a request surface: no.** _(Set to `yes` if this repo treats external PRs as feature requests, and triage
them as issues.)_
