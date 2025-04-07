# CI/CD Release Workflow

---

## Workflows

- To trigger a release, use [rc.yml](workflows/rc.yml) workflow.
  - It will need specific commit SHA in long format to start the workflow.
  - All the commits until that specific commit sha will be included in the release.
  - Checks version from `Cargo.toml` and validates if it needs to creates a new release branch. If there is a release branch that already exists for the same version in `Cargo.toml` the workflow will fail.
  - Release branch is created in this pattern `release-v<version>`.

- Second workflow [release-please.yml](workflows/release-please.yml) will get triggered on push to release branch automatically.
  - This workflow checks if there is any "higher versioned" branches than the current one since this workflow will be triggered for any pushes ( eg. hotfixes ).
  - We use [release-please](https://github.com/googleapis/release-please) for managing releases. If there are no "higher versioned" branches release-please step will be triggered.
  - Release please automatically creates a PR with Changelog notes to release branch which keeps track of all commits in that release branch and adds a label `autorelease: pending`. It uses [config](release-please/.config.json) & [manifest](release-please/manifest.json) files to generate changelog and track versions. If there are any changes to `Cargo.lock` that commit is pushed to the PR.
  - Once approved merge the PR. On merging `release-please` automatically creates a github release with changelog notes & tags the release with that version.
  - Workflow has a step to unlock conversation in the now closed PR so that release-please can post a comment and update the label `autorelease: tagged`.
  - SBOM generation & Docker build and push jobs are triggered.

- If everything looks good post release, raise a PR and merge the `release-v<version>` branch to main (manual step for now).
