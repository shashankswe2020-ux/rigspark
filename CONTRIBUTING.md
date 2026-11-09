# Contributing to RigSpark

Thanks for helping. RigSpark is community-built: bug fixes, tests, catalog entries,
docs and features are all welcome. A paid edition may ship separately later; this
repository stays open source under the MIT licence.

## Licence of your contribution

RigSpark is released under the [MIT licence](LICENSE). By opening a pull request you
agree that your contribution is licensed under the same MIT terms, so anyone,
including the maintainers, can use, modify and redistribute it.

### Sign off your commits

Every commit needs a `Signed-off-by` line. It certifies the
[Developer Certificate of Origin 1.1](https://developercertificate.org/): that you
wrote the change, or otherwise have the right to submit it under the project's licence.

```bash
git commit -s -m "fix(doctor): warn when free disk is low"
```

`-s` adds `Signed-off-by: Your Name <you@example.com>` using your git name and email.
To sign off commits you already made: `git rebase --signoff main`.

## Where to start

- Issues labelled [`good first issue`](https://github.com/rigspark/rigspark/labels/good%20first%20issue)
  are small and self-contained, and most say which file to change.
- Issues labelled [`help wanted`](https://github.com/rigspark/rigspark/labels/help%20wanted)
  are open for anyone; comment first on the larger ones so work isn't duplicated.
- New models go through the **Request a model** issue form; every entry needs pinned,
  integrity-checked weights.
- For a new feature, open an issue first. Some areas need a short design before code.

## Set up

RigSpark is a Rust workspace pinned to Rust 1.98.1 (`rust-toolchain.toml`; `rustup`
installs it automatically). There is no Node.js or TypeScript tooling.

```bash
git clone https://github.com/rigspark/rigspark
cd rigspark
cargo build --workspace --locked
cargo rigspark recommend        # run the CLI from source
cargo rigspark gui              # run the GUI from source
```

## Before you open a pull request

CI runs these checks; run them locally first:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked -- --test-threads=2
cargo build --workspace --locked
cargo native-retirement          # fails if Node/TypeScript tooling comes back
```

The full test suite is slow. While you iterate, run one crate's tests, for example
`cargo test -p rigspark-runtime --locked`.

Pull requests also get a mutation-testing check on the lines they change. If it reports
a surviving mutant, add a test that fails when that line is changed.

## House rules

These are not negotiable; reviews check them.

- **Honest numbers.** If a figure can't be sourced (speed, bandwidth, quality), show
  `unknown`. Never invent a number or an estimate.
- **Deterministic advice.** Recommendations make no network calls and use the cited,
  offline data in `crates/rigspark-core/data/`.
- **Integrity, fail-closed.** Weights are verified against a pinned digest before they
  are served. Unverified weights are refused.
- **Loopback only.** Servers bind `127.0.0.1`. Nothing listens on the network.
- **Validate all input.** CLI arguments, catalog JSON, API responses and files go
  through typed `serde` models and explicit checks. Errors are typed, not sentinel values.
- **No unsafe code.** The workspace forbids `unsafe`.

## Tests

- Write the test first. For a bug, add a failing test, then fix it.
- Never spawn real Ollama, download models or call real endpoints in tests. Inject the
  network, filesystem, clock and process boundaries instead, and don't change process
  environment variables in tests that run in parallel.
- Prefer the lowest level that captures the behaviour: unit, then integration
  (`crates/<crate>/tests/*.rs`), then end-to-end.

## Pull requests

- Keep each pull request to one change, with a title in the
  [Conventional Commits](https://www.conventionalcommits.org/) style
  (`fix(gui): ...`, `feat(runtime): ...`, `docs: ...`).
- Say what changed, why, and how you tested it. Add a line to `CHANGELOG.md` under
  **Unreleased** for anything a user would notice.
- Screenshots help for GUI changes, at desktop and phone width.

## Security issues

Please don't open a public issue for a vulnerability. Report it privately through
GitHub's **Report a vulnerability** button on the repository's Security tab.

## Questions

Open an issue with the `question` label.
