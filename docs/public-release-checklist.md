# Public repository checklist

Complete this checklist when changing the GitHub repository from private to
public. Visibility changes expose the full reachable Git history, branches,
tags, releases, issues, pull requests, and Actions logs—not only the current
working tree.

## Before changing visibility

- [ ] Run a secret scanner across all Git refs and inspect deleted files and
      Actions logs for credentials or private deployment data.
- [ ] Review commit author email addresses and historical domains for privacy.
- [ ] Confirm every public release asset and container image is intended for
      redistribution and has matching checksums and license metadata.
- [ ] Review collaborators, deploy keys, webhooks, environments, Actions
      secrets, and repository variables.
- [ ] Confirm `main` is green and `make check` plus
      `scripts/validate-production-docs.sh` pass locally.

## Immediately after changing visibility

- [ ] Enable private vulnerability reporting, secret scanning, push protection,
      Dependabot alerts, and Dependabot security updates under Security settings.
- [ ] Protect `main` (or add a ruleset): require pull requests, required CI
      checks, conversation resolution, and protection against force pushes and
      deletion. Allow the release automation only the access it needs.
- [ ] Verify the repository description, topics, license display, issue forms,
      security policy, release links, and README badges as a signed-out visitor.
- [ ] Verify the latest package downloads and multi-platform Docker image remain
      accessible without repository access.
- [ ] Open a test pull request to confirm pinned Actions and Dependabot updates
      work with the public repository's token permissions.
