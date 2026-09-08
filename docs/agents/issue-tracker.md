# Issue tracker: GitHub

Issues and PRDs live as GitHub issues in this repository. Use the `gh` CLI for tracker operations, resolving the repository from its Git remote.

- When a skill says "publish to the issue tracker", create a GitHub issue.
- When a skill says "fetch the relevant ticket", use `gh issue view <number> --comments`; include labels when assessing its state.
- Use [Triage labels](triage-labels.md) as the authoritative role-to-label mapping.
- For map/child tickets, claiming, and blockers, read [Wayfinding](wayfinding.md).

## Pull requests as a triage surface

**PRs as a request surface: no.**

GitHub shares one number space across issues and PRs. Resolve a bare reference with `gh pr view <number>` and fall back to `gh issue view <number>`; do not assume its type.
