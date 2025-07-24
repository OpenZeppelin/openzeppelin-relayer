# Contributing

Thank you for your interest in contributing to the OpenZeppelin Relayer project! This document provides guidelines to ensure your contributions are effectively integrated into the project.

There are many ways to contribute, regardless of your experience level. Whether you're new to Rust or a seasoned expert, your help is invaluable. Every contribution matters, no matter how small, and all efforts are greatly appreciated. This document is here to guide you through the process. Don’t feel overwhelmed—it’s meant to support and simplify your contribution journey.

- [Contributing](#contributing)
  - [Communication](#communication)
  - [Development Workflow](#development-workflow)
  - [GitHub workflow](#github-workflow)
    - [1. Fork in the cloud](#1-fork-in-the-cloud)
    - [2. Clone fork to local storage](#2-clone-fork-to-local-storage)
    - [3. Create a Working Branch](#3-create-a-working-branch)
    - [4. Keep your branch in sync](#4-keep-your-branch-in-sync)
    - [5. Pre Commit Hooks](#5-pre-commit-hooks)
    - [6. Commit Your Changes](#6-commit-your-changes)
    - [7. Push to GitHub](#7-push-to-github)
    - [8. Create a Pull Request](#8-create-a-pull-request)
    - [Get a code review](#get-a-code-review)
    - [Squash commits](#squash-commits)
    - [Merging a commit](#merging-a-commit)
    - [Reverting a commit](#reverting-a-commit)
    - [Opening a Pull Request](#opening-a-pull-request)
  - [Code Review](#code-review)
  - [Best practices](#best-practices)
  - [Coding Standards](#coding-standards)
  - [Testing](#testing)
  - [Security](#security)
  - [Documentation](#documentation)
  - [Issue and Pull Request Labeling Guidelines](#issue-and-pull-request-labeling-guidelines)
    - [1. Area Labels (`A-`)](#1-area-labels-a-)
    - [2. Type Labels (`T-`)](#2-type-labels-t-)
    - [3. Priority Labels (`P-`)](#3-priority-labels-p-)
    - [4. Status Labels (`S-`)](#4-status-labels-s-)
    - [5. Difficulty Labels (`D-`)](#5-difficulty-labels-d-)
    - [6. Other Useful Labels](#6-other-useful-labels)
    - [How to Use These Labels](#how-to-use-these-labels)
  - [License](#license)
  - [Code of Conduct](#code-of-conduct)

OpenZeppelin Relayer is open source and welcomes contributions from the community.

As a potential contributor, your changes and ideas are welcome at any hour of the day or night, weekdays, weekends, and holidays.
Please do not ever hesitate to ask a question or send a pull request.

Beginner focused information can be found below in [Open a Pull Request](#opening-a-pull-request) and [Code Review](#code-review).

## Communication

- [CODEOWNERS](./CODEOWNERS)
- [Telegram](t.me/openzeppelin_tg/2)
- [Website](https://openzeppelin.com/)
- [Blog](https://blog.openzeppelin.com/)
- [X](https://x.com/OpenZeppelin)

## Development Workflow

1. **Install Sodium**:
   - Install stable libsodium version from [here](https://download.libsodium.org/libsodium/releases/).
   - Follow steps to install libsodium from the [libsodium installation guide](https://doc.libsodium.org/installation).

2. **Set Up Development Environment**:
   - Install dependencies:

     ```sh
     cargo build
     ```

   - Set up environment variables:

     ```sh
     cp .env.example .env
     ```

3. **Run Tests**:
   - Unit tests:

     ```sh
     cargo test
     ```

   - Integration tests:

     ```sh
     cargo test integration
     ```

    > Note: If you run into any issues with the tests, run the tests with `RUST_TEST_THREADS=1` to avoid any racing conditions between tests.


4. **Configure Pre commit Hooks**:

   - Install & Configure Pre-Commit hooks

    ```sh

      # Use <pipx install pre-commit> if you prefer to install it globally

      pip install pre-commit
      pre-commit install --install-hooks -t commit-msg -t pre-commit -t pre-push
    ```

    > Note: If you run into issues with pip install, you may need [pipx](https://github.com/pypa/pipx?tab=readme-ov-file#install-pipx) to install pre-commit globally.

## GitHub workflow

### 1. Fork in the cloud

- Visit <https://github.com/openzeppelin/openzeppelin-relayer>
- Click `Fork` button (top right) to establish a cloud-based fork.

### 2. Clone fork to local storage

In your shell, define a local working directory as `working_dir`.

```sh
export working_dir="${HOME}/repos" # Change to your preferred location for source code
```

Set `user` to match your github profile name:

```sh
export user=<your github profile name>
```

Create your clone:

```sh
mkdir -p $working_dir
cd $working_dir
git clone https://github.com/$user/openzeppelin-relayer.git
# or: git clone git@github.com:$user/openzeppelin-relayer.git

cd $working_dir/openzeppelin-relayer
git remote add upstream https://github.com/openzeppelin/openzeppelin-relayer.git
# or: git remote add upstream git@github.com:openzeppelin/openzeppelin-relayer.git

# Never push to upstream main
git remote set-url --push upstream no_push

# Confirm that your remotes make sense:
git remote -v
```

### 3. Create a Working Branch

Get your local main up to date.

```sh
cd $working_dir/openzeppelin-relayer
git fetch upstream
git checkout main
git rebase upstream/main
```

Create your new branch.

```sh
git checkout -b myfeature
# or git switch -c myfeature
```

You may now edit files on the `myfeature` branch.

### 4. Keep your branch in sync

You will need to periodically fetch changes from the `upstream`
repository to keep your working branch in sync.

Make sure your local repository is on your working branch and run the
following commands to keep it in sync:

```sh
git fetch upstream
git rebase upstream/main
```

Please don't use `git pull` instead of the above `fetch` and
`rebase`. Since `git pull` executes a merge, it creates merge commits. These make the commit history messy
and violate the principle that commits ought to be individually understandable
and useful (see below).

You might also consider changing your `.git/config` file via
`git config branch.autoSetupRebase always` to change the behavior of `git pull`, or another non-merge option such as `git pull --rebase`.

### 5. Pre Commit Hooks

We use pre-commit hooks to ensure that all code is formatted and linted correctly.

We assume you already have `pipx` installed. If not, you can install it by following documentation [here](https://pipx.pypa.io/stable/installation/).

To install and configure pre-commit hooks, run the following commands:

```sh
# Use <pipx install pre-commit> if you prefer to install it globally
pip install pre-commit
pre-commit install --install-hooks -t commit-msg -t pre-commit -t pre-push
```

This will install pre-commit hooks that will run on every commit and push. The hooks will check for linting, formatting, and other issues in your code.

### 6. Commit Your Changes

You will probably want to regularly commit your changes. It is likely that you will go back and edit,
build, and test multiple times. After a few cycles of this, you might
[amend your previous commit](https://www.w3schools.com/git/git_amend.asp).

We use signed commits enforcement as a best practice. Make sure to sign your commits. This is a requirement for all commits.
You can read more about signing commits [here](https://docs.github.com/en/authentication/managing-commit-signature-verification/signing-commits). Also see telling git about your signing key [here](https://docs.github.com/en/authentication/managing-commit-signature-verification/telling-git-about-your-signing-key).

Once you enable gpg signing globally in git, all commits will be signed by default. If you want to sign a commit manually, you can use the `-S` flag with the `git commit` command.

```sh
git commit
```

### 7. Push to GitHub

When your changes are ready for review, push your working branch to
your fork on GitHub.

```sh
git push -f <your_remote_name> myfeature
```

### 8. Create a Pull Request

- Visit your fork at `https://github.com/<user>/openzeppelin-relayer`
- Click the **Compare & Pull Request** button next to your `myfeature` branch.

_If you have upstream write access_, please refrain from using the GitHub UI for
creating PRs, because GitHub will create the PR branch inside the main
repository rather than inside your fork.

### Get a code review

Once your pull request has been opened it will be assigned to one or more
reviewers.  Those reviewers will do a thorough code review, looking for
correctness, bugs, opportunities for improvement, documentation and comments,
and style.

Commit changes made in response to review comments to the same branch on your
fork.

Very small PRs are easy to review.  Very large PRs are very difficult to review.

### Squash commits

After a review, we automatically squash commits when merging a PR. This means that all commits in your PR will be combined into a single commit in the main branch. This is done to keep the commit history clean and easy to read.

### Merging a commit

Once you've received review and approval, your commits are squashed, your PR is ready for merging.

Merging happens automatically after both a Reviewer and Approver have approved the PR. If you haven't squashed your commits, they may ask you to do so before approving a PR.

### Reverting a commit

In case you wish to revert a commit, use the following instructions.

_If you have upstream write access_, please refrain from using the
`Revert` button in the GitHub UI for creating the PR, because GitHub
will create the PR branch inside the main repository rather than inside your fork.

- Create a branch and sync it with upstream.

  ```sh
  # create a branch
  git checkout -b myrevert

  # sync the branch with upstream
  git fetch upstream
  git rebase upstream/main
  ```

- If the commit you wish to revert is a _merge commit_, use this command:

  ```sh
  # SHA is the hash of the merge commit you wish to revert
  git revert -m 1 <SHA>
  ```

  If it is a _single commit_, use this command:

  ```sh
  # SHA is the hash of the single commit you wish to revert
  git revert <SHA>
  ```

- This will create a new commit reverting the changes. Push this new commit to your remote.

  ```sh
  git push <your_remote_name> myrevert
  ```

- Finally, [create a Pull Request](#8-create-a-pull-request) using this branch.

### Opening a Pull Request

Pull requests are often called a "PR".
OpenZeppelin Relayer generally follows the standard [github pull request](https://help.github.com/articles/about-pull-requests/) process, but there is a layer of additional specific differences:

Common new contributor PR issues are:

- Dealing with test cases which fail on your PR, unrelated to the changes you introduce.
- Include mentions (like @person) and [keywords](https://help.github.com/en/articles/closing-issues-using-keywords) which could close the issue (like fixes #xxxx) in commit messages.

## Code Review

As a community we believe in the value of code review for all contributions.
Code review increases both the quality and readability of our codebase, which
in turn produces high quality software.

As a community we expect that all active participants in the
community will also be active reviewers.

There are two aspects of code review: giving and receiving.

To make it easier for your PR to receive reviews, consider the reviewers will need you to:

- Write [good commit messages](https://chris.beams.io/posts/git-commit/)
- Break large changes into a logical series of smaller patches which individually make easily understandable changes, and in aggregate solve a broader issue
- Label PRs: to do this read the messages the bot sends you to guide you through the PR process

Reviewers, the people giving the review, are highly encouraged to revisit the [Code of Conduct](./CODE_OF_CONDUCT.md) and must go above and beyond to promote a collaborative, respectful community.
When reviewing PRs from others [The Gentle Art of Patch Review](http://sage.thesharps.us/2014/09/01/the-gentle-art-of-patch-review/) suggests an iterative series of focuses which is designed to lead new contributors to positive collaboration without inundating them initially with nuances:

- Is the idea behind the contribution sound?
- Is the contribution architected correctly?
- Is the contribution polished?

Note: if your pull request isn't getting enough attention, you can contact us on [Telegram](t.me/openzeppelin_tg/2) to get help finding reviewers.

## Best practices

- Write clear and meaningful git commit messages.
- If the PR will _completely_ fix a specific issue, include `fixes #123` in the PR body (where 123 is the specific issue number the PR will fix. This will automatically close the issue when the PR is merged.
- Make sure you don't include `@mentions` or `fixes` keywords in your git commit messages. These should be included in the PR body instead.
- When you make a PR for small change (such as fixing a typo, style change, or grammar fix), please squash your commits so that we can maintain a cleaner git history.
- Make sure you include a clear and detailed PR description explaining the reasons for the changes, and ensuring there is sufficient information for the reviewer to understand your PR.
- Additional Readings:
  - [chris.beams.io/posts/git-commit/](https://chris.beams.io/posts/git-commit/)
  - [github.com/blog/1506-closing-issues-via-pull-requests](https://github.com/blog/1506-closing-issues-via-pull-requests)
  - [davidwalsh.name/squash-commits-git](https://davidwalsh.name/squash-commits-git)
  - [https://mtlynch.io/code-review-love/](https://mtlynch.io/code-review-love/)

## Coding Standards

- Use **Rust 2021 edition**, version `1.86` or later.
- Follow the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/).
- Run pre-commit hooks on your code to ensure code quality.

## Testing

Testing is the responsibility of all contributors as such all contributions must pass existing tests and include new tests when applicable:

1. Write tests for new features or bug fixes.
2. Run the test suite:

   ```sh
   cargo test
   ```

3. Ensure no warnings or errors.
4. Make sure you have test coverage for your code. You can run ```RUST_TEST_THREADS=1 cargo llvm-cov --locked --html --open``` to open the coverage report in the browser and verify the percentages for your code. Make sure to have a minimum of 80% coverage.

## Security

- Follow the stated [Security Policy](SECURITY.md).

## Documentation

- Pre-requisites:

  - You need `antora` `site-generator` and `mermaid` extension to generate the documentation.

  - You can directly install these dependencies by running `cd docs && npm i --include dev`. If you want to install them manually, you can follow the steps mentioned below.
  - Install `antora` locally, you can follow the steps mentioned [here](https://docs.antora.org/antora/latest/install/install-antora/#install-dir), if you already have you can skip this step.
    > Note: If you want to install globally, you can run: <br/> `npm install -g @antora/cli@3.1 @antora/site-generator@3.1 @sntke/antora-mermaid-extension`
  - Verify the installation by running `antora --version` or by running `npx antora --version` if you installed it locally.

- To generate documentation locally, run the following command:

  ```sh
  cargo make rust-antora
  ```

- Site will be generated in `docs/build/site/openzeppelin-relayer/<version>/` directory.

- To view the documentation, open the `docs/build/site/openzeppelin-relayer/<version>/index.html` in your browser.

## Issue and Pull Request Labeling Guidelines

To ensure clarity and effective project management, we use a structured labeling system for issues and pull requests. Below are the label categories and their purposes:

### 1. Area Labels (`A-`)

These labels identify the part of the project the issue or PR pertains to:

**`A-arch`**: High-level architectural concerns or changes.
**`A-clients`**: Issues related to blockchain clients (e.g., EVM, Solana, Stellar).
**`A-pipeline`**: Signer, Provider, and global Relayer services and CI pipelines.
**`A-configs`**: Issues related to `.env` files, relayer configuration, or network settings.
**`A-tests`**: Test setup and integration.
**`A-docs`**: Updates or fixes to project documentation.
**`A-deps`**: Pull requests that update a dependency file.

---

### 2. Type Labels (`T-`)

These labels describe the nature of the issue or PR:

**`T-bug`**: Indicates a bug report.
**`T-feature`**: Suggests a new feature or enhancement.
**`T-task`**: General tasks or chores (e.g., refactoring, cleanup).
**`T-documentation`**: Issues or PRs related to documentation updates.
**`T-performance`**: Performance optimizations or bottlenecks.
**`T-security`**: Security vulnerabilities or related fixes.

---

### 3. Priority Labels (`P-`)

Define the priority level for addressing issues:

**`P-high`**: Critical tasks or blockers.
**`P-medium`**: Important but not urgent.
**`P-low`**: Low-priority or non-urgent tasks.

---

### 4. Status Labels (`S-`)

Labels to track the workflow status of an issue:

**`S-needs-triage`**: Requires initial triage or categorization.
**`S-in-progress`**: Actively being worked on.
**`S-blocked`**: Blocked by another issue or dependency.
**`S-needs-review`**: Awaiting review (code or design).
**`S-closed`**: Completed and closed issues.

---

### 5. Difficulty Labels (`D-`)

Indicate the complexity or effort required to address the issue:

**`D-easy`**: Beginner-friendly tasks.
**`D-medium`**: Intermediate-level tasks.
**`D-hard`**: Complex or advanced issues.

---

### 6. Other Useful Labels

**`good-first-issue`**: Beginner-friendly, low-complexity issues to help new contributors.
**`help-wanted`**: Issues where community contributions are welcome.
**`discussion`**: Requires community or team input.
**`wontfix`**: This will not be worked on.
**`duplicate`**: This issue or pull request already exists.

---

### How to Use These Labels

When creating or triaging an issue or PR, apply the appropriate labels from the categories above. This helps maintain clarity, improve collaboration, and ensure smooth workflow management for all contributors.

If you are unsure which label to apply, feel free to leave the issue or PR with the **`S-needs-triage`** label, and a maintainer will review it.

## License

By contributing to this project, you agree that your contributions will be licensed under the [AGPL-3.0 License](LICENSE).

## Code of Conduct

This project and everyone participating in it is governed by the [Code of Conduct](CODE_OF_CONDUCT.md). By participating, you are expected to uphold this code. Please report any unacceptable behavior on [Telegram](t.me/openzeppelin_tg/2).`
