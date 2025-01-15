# OpenZeppelin Monitor


### Developer setup

1. Run `chmod +x .githooks/*` to make the git hooks executable.
2. Run `git config core.hooksPath .githooks` to setup git hooks for linting and formatting.
3. Run `rustup toolchain install nightly` to install the nightly toolchain.
4. Run `rustup component add rustfmt --toolchain nightly` to install rustfmt for the nightly toolchain.