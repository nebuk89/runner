<p align="center">
  <img src="docs/res/github-graph.png">
</p>

# GitHub Actions Runner

[![Actions Status](https://github.com/actions/runner/workflows/Runner%20CI/badge.svg)](https://github.com/actions/runner/actions)

The runner is the application that runs a job from a GitHub Actions workflow. It is used by GitHub Actions in the [hosted virtual environments](https://github.com/actions/virtual-environments), or you can [self-host the runner](https://help.github.com/en/actions/automating-your-workflow-with-github-actions/about-self-hosted-runners) in your own environment.

## Get Started

For more information about installing and using self-hosted runners, see [Adding self-hosted runners](https://help.github.com/en/actions/automating-your-workflow-with-github-actions/adding-self-hosted-runners) and [Using self-hosted runners in a workflow](https://help.github.com/en/actions/automating-your-workflow-with-github-actions/using-self-hosted-runners-in-a-workflow)

Runner releases:

![win](docs/res/win_sm.png) [Pre-reqs](docs/start/envwin.md) | [Download](https://github.com/actions/runner/releases)  

![macOS](docs/res/apple_sm.png)  [Pre-reqs](docs/start/envosx.md) | [Download](https://github.com/actions/runner/releases)  

![linux](docs/res/linux_sm.png)  [Pre-reqs](docs/start/envlinux.md) | [Download](https://github.com/actions/runner/releases)

### Note

Thank you for your interest in this GitHub repo, however, right now we are not taking contributions. 

We continue to focus our resources on strategic areas that help our customers be successful while making developers' lives easier. While GitHub Actions remains a key part of this vision, we are allocating resources towards other areas of Actions and are not taking contributions to this repository at this time. The GitHub public roadmap is the best place to follow along for any updates on features we’re working on and what stage they’re in.

We are taking the following steps to better direct requests related to GitHub Actions, including:

1. We will be directing questions and support requests to our [Community Discussions area](https://github.com/orgs/community/discussions/categories/actions)

2. High Priority bugs can be reported through Community Discussions or you can report these to our support team https://support.github.com/contact/bug-report.

3. Security Issues should be handled as per our [SECURITY.md](https://github.com/actions/runner?tab=security-ov-file)

We will still provide security updates for this project and fix major breaking changes during this time.

You are welcome to still raise bugs in this repo.

## Rust Runner

A full Rust rewrite of the GitHub Actions Runner lives in the [`rust/`](rust/) directory. It is a Cargo workspace containing six crates that mirror the original C# project structure.

### Prerequisites

| Requirement | Minimum version |
|-------------|-----------------|
| [Rust toolchain](https://rustup.rs/) | **1.75** (edition 2021) |
| A C linker (`cc` / MSVC) | any recent version |
| OpenSSL headers **or** `rustls` (default — no system OpenSSL needed) | — |
| Docker (only if you plan to run container actions) | 20.10+ |

### Project layout

```
rust/
├── Cargo.toml                  # Workspace root
└── crates/
    ├── runner-sdk/             # Foundation: process invoker, HTTP client, utilities
    ├── runner-common/          # Shared services: HostContext, config, IPC, logging
    ├── runner-worker/          # Execution engine: job/step runners, action handlers
    ├── runner-listener/        # Main process: message loop, job dispatch, self-update
    ├── runner-plugins/         # Artifact upload/download plugins
    └── runner-plugin-host/     # Out-of-process plugin executor
```

### Build

```bash
# Debug build (fast compile, unoptimised)
cd rust
cargo build

# Release build (optimised)
cargo build --release
```

The two main binaries are produced at:

| Binary | Path (release) | Purpose |
|--------|---------------|---------|
| `Runner.Listener` | `target/release/Runner.Listener` | Main runner process — configure, listen, dispatch |
| `Runner.Worker` | `target/release/Runner_Worker` | Worker process spawned per-job |

### Run the tests

```bash
cd rust
cargo test
```

### Configure and run (self-hosted)

```bash
# 1. Configure the runner (interactive)
./target/release/Runner.Listener configure \
  --url https://github.com/<owner>/<repo> \
  --token <REGISTRATION_TOKEN>

# 2. Start listening for jobs
./target/release/Runner.Listener run
```

The same flags from the C# runner are supported:

| Flag | Description |
|------|-------------|
| `--name <NAME>` | Runner name (default: hostname) |
| `--work <DIR>` | Work directory (default: `_work`) |
| `--labels <L1,L2>` | Comma-separated extra labels |
| `--runnergroup <GROUP>` | Runner group |
| `--replace` | Replace an existing runner with the same name |
| `--ephemeral` | Register as an ephemeral (single-job) runner |
| `--unattended` | Skip interactive prompts |
| `--disableupdate` | Disable automatic self-update |
| `--check` | Run connectivity diagnostics only |

### Cross-compilation

Rust makes cross-compilation straightforward. Add the target and build:

```bash
rustup target add x86_64-unknown-linux-gnu
cargo build --release --target x86_64-unknown-linux-gnu

rustup target add aarch64-unknown-linux-gnu
cargo build --release --target aarch64-unknown-linux-gnu

rustup target add x86_64-apple-darwin
cargo build --release --target x86_64-apple-darwin
```

### Formatting

```bash
cd rust
cargo fmt --all
```

### Linting

```bash
cd rust
cargo clippy --workspace --all-targets
```
