# Branches and pull requests

`main` contains the current release code, including fixes for that release. `dev`
is the integration branch for the next release. Tags such as `v0.1.0-beta.1`
identify immutable release snapshots; moving `main` does not change an old tag.
A beta tag is still a release, not a stability promise.

| Work | Branch from | Pull request target |
| --- | --- | --- |
| Feature, ordinary fix or documentation for the next release | `dev` | `dev` |
| Urgent fix for the current release | `main` | `main`, then merge the fix forward into `dev` |
| Release promotion | `dev` | `main`, after review and checks |

Use short-lived branches, for example `feature/local-import` or
`fix/model-registration`. Keep ordinary PRs focused. A release promotion should
preserve the reviewed history; use a merge commit rather than squash-merging the
whole integration branch. Tag the reviewed commit on `main` and merge release
fixes/version changes forward into `dev` so the lines do not drift.

CI runs on pushes to `main` and `dev`, and on pull requests. Repository maintainers
should configure GitHub rulesets for both branches to require PR review and the
source-validation check, and block force pushes/deletion. These are repository
settings; the files here do not enable them automatically. The PR template asks
contributors to choose the appropriate target.
