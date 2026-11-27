# Redlib Audit Remediation, Slice 7 — Task List

- **Status:** Complete except the `flake.nix` toolchain pin (blocked, see Task 4)
- **Author:** Argi
- **Created:** 2026-11-28
- **Language:** GitHub Actions YAML, Dockerfile, Compose, Nix, Cargo, justfile (no Rust source changes)
- **Source:** [plan.md](./plan.md), Slice 7 (S7): Container Pipeline, CI Hardening, Supply-Chain Pinning & Dev Ergonomics

## Summary

Every push to `main` builds the musl binaries twice, and the container workflow publishes images whether or not the build passed, from a binary it downloads from `releases/latest` (which ignores the draft release the build just made). The root `Dockerfile` cannot build under `compose.dev.yaml` or `heroku.yml`, every container healthcheck calls `wget` (absent from the Ubuntu image) and the PR workflow retries tests ten times without `--locked`. These tasks fix the pipeline, move every healthcheck to `redlib --healthcheck`, pin the supply chain and add the missing dev tooling. Each change is checked by running the tool it configures (actionlint, shellcheck, Podman builds and healthchecks, `just`), because a Rust test could only assert source text.

## Starting Context

Existing code the tasks build on (verified against `21e354f`, the Slice 6 head; do not re-explore):

- `src/main.rs:179-208` — `--healthcheck` flag. It returns before `config::CONFIG` loads, probes `/healthz` through `health::probe` (`src/health.rs:71`, 5 s per address, loopback for wildcard binds) and exits 0 (`ok`) or 1 (`unhealthy: <reason>` on stderr). `/healthz` answers 200 only with a live OAuth token.
- `.github/workflows/build-artifacts.yaml` ("Release Build") and `main-rust.yml` both build musl on every push to `main`; `main-docker.yml` runs on `workflow_run: [completed]` with no `conclusion` check and uses `github.sha`, which for a `workflow_run` is the default-branch tip, not the commit that was built.
- `Dockerfile` downloads `releases/latest/download/redlib-${TARGET}.tar.gz` with `curl -L | tar`; `Dockerfile.alpine` (rust-musl-cross builder, `alpine:3.23`) and `Dockerfile.ubuntu` (`rust:slim-bookworm` to `ubuntu:noble`) build from source.
- `scripts/build-musl-docker.sh <target>` builds the `Dockerfile.alpine` builder stage and extracts `target/<target>/release/redlib`.
- `tests/js/*.test.js` run with `node --test tests/js/`; live-Reddit tests are `#[ignore = "requires live Reddit"]`.

Conventions and gotchas:

- The working tree is CRLF (`core.autocrlf=true`); the index is LF. Container builds use an LF export (`git archive HEAD` plus the modified files) in `D:/src/redlib-scratch/s7`, never the CRLF tree.
- Upstream workflows on `argium/redlib` have never run, and nothing is pushed, so no workflow here is run on GitHub. Evidence is `actionlint`, `shellcheck`, local Podman builds and `just`.
- Third-party actions that this slice adds or edits are pinned to a full commit SHA with the release tag in a comment, at the latest stable release (checked with `gh api` on 2026-11-28). Actions it does not edit (`docker/*`) stay as they are.
- Podman here has 2 GiB of RAM, 8 CPUs and no Docker daemon. Reddit answers 403 from this network, which can stop a container reaching `healthy`.
- No new crates; no Rust source changes.

## Tasks

### Task 1: Containers build, run as non-root and report health through `redlib --healthcheck` (M7.2)

#### Problem

`Dockerfile` has no default `TARGET`, uses EOL `alpine:3.19` and pipes a 404 page into `tar`, so `compose.dev.yaml` and `heroku.yml` (which build `.`) fail. `Dockerfile.ubuntu` runs `wget`, which `ubuntu:noble` lacks, so it is `unhealthy` forever. `compose.yaml`, `compose.dev.yaml` and `redlib.container` poll `/settings` through `wget` every five minutes with no `start_period`.

#### Solution

- `Dockerfile` becomes the release packaging image: it unpacks the checksum-verified `redlib-<target>.tar.gz` from `dist/` onto `alpine:${ALPINE_VERSION}` (same pinned minor release as `Dockerfile.alpine`, default `TARGET=x86_64-unknown-linux-musl`). It no longer touches the network.
- `compose.dev.yaml` and `heroku.yml` build `Dockerfile.alpine` (from source, amd64).
- All three Dockerfiles, both compose files and `redlib.container` use `redlib --healthcheck` with a 30 s interval, 10 s timeout and 30 s start period.
- `compose.dev.yaml` loses the obsolete `version:` key and its wrong file-name comment.

#### Notes

- **Location:** `Dockerfile`, `Dockerfile.alpine`, `Dockerfile.ubuntu`, `compose.yaml`, `compose.dev.yaml`, `heroku.yml`, `redlib.container`, `.gitignore`

- **Verify:** `podman build --format docker -f Dockerfile.alpine .` and `-f Dockerfile.ubuntu .` succeed, a run of each image reaches `healthy` (or the failure is the network, with `redlib --healthcheck` exiting 1 with its reason), and the root `Dockerfile` builds from a `dist/` made from the Alpine binary.

- Boundary: real Podman builds and runs. Not verified: Docker itself, multi-arch (arm64/armv7) builds, Heroku.

---

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Red** — before the change (HEAD export): `podman build -f Dockerfile .` fails at `curl -L … | tar xz` with `tar: invalid magic`; `podman run ubuntu:noble wget --version` fails with `executable file wget not found` (the Ubuntu healthcheck command).

**Implementation guidance (green)**

- `Dockerfile`: `FROM alpine:${ALPINE_VERSION} AS unpack`, `COPY dist/redlib-${TARGET}.tar.gz dist/redlib-${TARGET}.tar.gz.sha256 /dist/`, `cd /dist && sha256sum -c … && tar xzf …`; release stage copies only the binary.
- `Dockerfile.*`: `HEALTHCHECK --interval=30s --timeout=10s --start-period=30s --retries=3 CMD ["redlib", "--healthcheck"]`.
- Compose: `test: ["CMD", "redlib", "--healthcheck"]`, `interval: 30s`, `timeout: 10s`, `start_period: 30s`, `retries: 3`. Quadlet: `HealthCmd=redlib --healthcheck`, `HealthStartPeriod=30s`.
- `dist/` goes in `.gitignore`; `.dockerignore` must not exclude it.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** None

- **Supports:** M7.2

</details>

- [x] Task 1 complete

---

### Task 2: One release build, and containers published only from that build's verified binary (M7.1)

#### Problem

`build-artifacts.yaml` and `main-rust.yml` build the musl target twice per push. `main-docker.yml` starts when "Release Build" completes in any state, tags images with the default-branch SHA and bakes in whatever `releases/latest` serves, so a failed or newer build publishes an older binary as `latest`.

#### Solution

- `build-artifacts.yaml` is the only build: per target it builds, packs `redlib-<target>.tar.gz` plus its `.sha256`, uploads both as a workflow artifact and to the draft release. A `publish-crate` job runs `cargo publish` on `release` events after the builds pass. `main-rust.yml` is deleted.
- `main-docker.yml` runs only when `workflow_run.conclusion == 'success'`, checks out `workflow_run.head_sha`, downloads that run's artifact, and passes `TARGET` to the verifying `Dockerfile`. Image tags use `head_sha`.

#### Notes

- **Location:** `.github/workflows/build-artifacts.yaml`, `.github/workflows/main-docker.yml`, `.github/workflows/main-rust.yml` (deleted)

- **Verify:** `actionlint` is clean; `shellcheck` is clean on every `run:` block; a local replay of the pack, checksum and `docker build -f Dockerfile` handoff succeeds, and a tampered tarball fails the build.

- Boundary: actionlint plus a local replay. Not verified: the workflows themselves (never run on GitHub), registry push, multi-arch.

---

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Red** — `actionlint` on HEAD: 6 findings (`actions/checkout@v3` too old ×4, SC2046 ×2 in `main-docker.yml`). Inspection: no `conclusion` check; two musl builds per push.

**Implementation guidance (green)**

- `softprops/action-gh-release` stays at its current pin (`v3.0.3` to `v3.0.4`, a patch update to the same SHA policy).
- The crates.io token stays in `secrets.CARGO_REGISTRY_TOKEN`, passed through `env`, not interpolated into the script.
- Job permissions are the minimum: `contents: write` only where the release is uploaded, `actions: read` for the artifact download.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 1

- **Supports:** M7.1

</details>

- [x] Task 2 complete

---

### Task 3: PR checks run what the release runs, without masking failures (M7.3)

#### Problem

`pull-request.yml` retries every test ten times, builds without `--locked`, has no cache, lints without `--all-targets`, uses `actions/checkout@v3`, never runs the JS tests, audits nothing and never builds the musl container, so a broken cross build is found after merge. `wreq` and `wreq-util` are caret-versioned release candidates, so a new RC can change TLS behaviour without a lockfile edit.

#### Solution

- Test, format, clippy (`--locked --all-targets -D warnings`), JS tests, `cargo audit` and an Alpine container smoke build as parallel jobs; `rust-cache` on the Rust jobs; read-only `contents` permission.
- `NEXTEST_RETRIES` is gone.
- `Cargo.toml` pins `wreq` and `wreq-util` with `=`.

#### Notes

- **Location:** `.github/workflows/pull-request.yml`, `Cargo.toml`

- **Verify:** `actionlint` clean; `git diff --exit-code Cargo.lock` after `cargo metadata --locked` and `cargo tree` before/after show identical versions; `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked` pass.

- Boundary: local equivalents of each CI step. Not verified: GitHub-hosted runners, `taiki-e/install-action` tool installs, cache behaviour.

---

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 2

- **Supports:** M7.3

</details>

- [x] Task 3 complete

---

### Task 4: Dev ergonomics: build context, devcontainer, Nix toolchain and `justfile` (M7.4)

#### Problem

`.dockerignore` sends `.git` into every build, so each commit invalidates the `COPY . ./` layer. The devcontainer runs `cargo build` on every start. `flake.nix` floats `rust-bin.stable.latest` and sets `doCheck = false`. There is no `justfile`.

#### Solution

- `.dockerignore` lists `.git`; `.devcontainer/devcontainer.json` drops `postStartCommand`.
- `justfile` defines `fmt`, `clippy`, `test` and `lint` (`default` lists the recipes).
- `flake.nix` pins the toolchain and enables checks, **if the pinned version can build the locked dependencies**. `Cargo.toml` says `rust-version = "1.81"`, but the locked dependencies need 1.89 (see the MSRV table below). `flake.nix` is left unchanged and reported as a blocker instead of bumping silently.

#### Notes

- **Location:** `.dockerignore`, `.devcontainer/devcontainer.json`, `justfile`, `flake.nix`

- **Verify:** `just --list && just lint` and `just test` exit 0.

- Not verified: Nix (not installed), the devcontainer itself.

---

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 3

- **Supports:** M7.4

</details>

- [ ] Task 4 complete (`flake.nix` toolchain pin and `doCheck` open; the rest is done)

---

### Task 5: Integration and boundary verification

#### Notes

- **Verify:** `actionlint` clean; `shellcheck` clean on the scripts and every workflow `run:` block; `just --list && just lint`, `just test`; the Podman builds and healthchecks from Task 1; `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked` and `node --test tests/js/` exit 0; the MSRV probe results are recorded.

- Boundary: local tools only. No GitHub, registry or Nix run.

- [x] Task 5 complete

---

## Acceptance Evidence

Scratch logs, exports and replays are in `D:/src/redlib-scratch/s7`.

### Checks run

- `actionlint` (v1.7.12, with shellcheck) on all workflows: 6 findings on `21e354f` (`actions/checkout@v3` too old ×4, SC2046 ×2 in `main-docker.yml`), clean after.
- `shellcheck` on `scripts/build-musl-docker.sh` (LF index version) clean. No script was changed; the other three scripts have old style warnings and are untouched.
- `docker-compose` v5.8.1 `config` (no daemon needed; neither `docker-compose` nor `podman-compose` is installed, so `podman compose` cannot run) accepts `compose.yaml` and `compose.dev.yaml`. On `21e354f` it warned that `version` in `compose.dev.yaml` is obsolete; no warning now.
- `just --list && just lint` (clippy `--locked --all-targets -D warnings`, `cargo fmt --check`) exit 0; `just test` exits 0: 218 library tests (20 live-Reddit ignored), 30 binary, 3 `healthcheck`, 2 `startup_config`, `node --test` 32 of 32.
- `cargo tree --locked` for `wreq`/`wreq-util` and `Cargo.lock` are identical before and after the pins.

### Containers (Podman 6.2.1, `--format docker` so `HEALTHCHECK` is kept; 2 GiB VM)

| Check | Before (`21e354f`) | After |
| --- | --- | --- |
| `podman build -f Dockerfile .` | fails at `curl -L … \| tar xz`: `tar: invalid magic` (a 404 body) | builds from a `dist/` tarball made in a Linux container with the workflow's `tar` and `sha256sum` commands; a tarball with one extra byte fails the build with `redlib-x86_64-unknown-linux-musl.tar.gz: FAILED` |
| `Dockerfile.alpine` | not run | builds (the musl cross build fits in 2 GiB) |
| `Dockerfile.ubuntu` | `podman run ubuntu:noble wget --version` → `executable file wget not found` (the old probe) | builds |
| Health, run with the compose settings `--read-only --user nobody --cap-drop ALL --security-opt no-new-privileges:true` | not run | all three images: `starting` at 10 s and 20 s, `healthy` at 30 s (OAuth succeeded from this network); `podman healthcheck run` exits 0; `redlib --healthcheck` inside prints `ok` |
| Negative control | | `redlib --healthcheck -p 9999` inside an image exits 1: `unhealthy: cannot reach [::1]:9999: … Connection refused` |
| `.dockerignore` | a build context copy shows `.git` | `.git` is gone, other files copied |

### Findings checked against the code

- **#11**: confirmed (`workflow_run` had no `conclusion` check, `github.sha` is the default-branch tip for `workflow_run`, the Dockerfile read `releases/latest`). Fixed in `main-docker.yml` and `Dockerfile`.
- **#12**: confirmed and fixed (default `TARGET`, supported Alpine, no network fetch, `compose.dev.yaml`/`heroku.yml` build from source, healthchecks).
- **#34 (wiring)**: `compose.yaml`, `compose.dev.yaml`, the three Dockerfiles and `redlib.container` (a sixth `wget` copy the plan did not list) now call `redlib --healthcheck`. The README documents the flag.
- **#36 (CI part)**: `NEXTEST_RETRIES` removed; live tests are `#[ignore]`d since Slice 1.
- **#37**: duplicate musl build removed; `--locked`, `rust-cache`, `clippy --all-targets`, current checkout and a PR container smoke build added. The RC pins are `=6.0.0-rc.31` and `=3.0.0-rc.14`, **not** the `rc.28`/`rc.10` named in the plan and in the old `Cargo.toml`, because `Cargo.lock` already holds rc.31 and rc.14 and the pins must not change it.
- **#55**: `.git` ignored, `postStartCommand` removed, `justfile` added. `flake.nix` pin and checks are open.

### MSRV probe (`cargo +<v> check --locked --all-targets`)

| Toolchain | Result |
| --- | --- |
| 1.81 | fails at manifest parse: `block-buffer 0.12.1` needs `edition2024` |
| 1.88 | fails: `uuid@1.28.0 requires rustc 1.89.0` |
| 1.89 | passes (`msrv-1.89.log`) |

`Cargo.toml` `rust-version = "1.81"` is wrong for the locked tree. The pin in `flake.nix` needs a decision: bump `rust-version` and pin 1.89, downgrade the locked dependencies, or leave the toolchain floating.

### Decisions for the orchestrator

- **`flake.nix`** is unchanged (toolchain pin and `doCheck = true`). Nix is not installed, so neither could be run.
- **`cargo deny check advisories`** on the locked tree fails today: RUSTSEC-2026-0258 (h2), -0391 (zstd), -0398 and -0403 (zstd-safe), -0433 (pulldown-cmark), -0592 (url), -0594 (cookie), plus unmaintained `bincode` (RUSTSEC-2025-0141) and `chrono` (-0543). The CI `audit` job is `continue-on-error: true` until those are cleared; making it blocking is one line.
- Releases now ship `redlib-<target>.tar.gz.sha256` (the SHA-256 of the tarball) instead of `redlib.sha512`/`redlib.sha256` of the raw x86_64 binary.

### Not verified, and why

- No workflow was run on GitHub (never run on the fork, nothing pushed): the `workflow_run` gate, artifact handoff with `download-artifact` v8 and `run-id`, `docker/*` actions, Quay push, crates.io publish and the `taiki-e/install-action` installs are checked only by `actionlint` and a local replay of pack, checksum and image build.
- arm64 and armv7 images (QEMU), Docker itself (Podman only), Heroku, Nix, the devcontainer.
- `docker/metadata-action` still labels images with `org.opencontainers.image.revision` from `github.sha` (the default branch tip) in a `workflow_run`; tags use the built commit.
- `docker/*` actions keep their old major versions (unrelated to this change); the actions this slice adds or edits are at the latest stable release with full SHAs: checkout v7.0.1 (newer than the plan's v4), upload-artifact v7.0.2, download-artifact v8.0.2, rust-cache v2.9.3, install-action v2.90.11, action-gh-release v3.0.4, `dtolnay/rust-toolchain` at the `stable` branch tip (the action publishes no releases).

### MSRV decision (orchestrator, 2026-11-28)

`rust-version = "1.81"` was already untrue before this slice: 1.81 cannot parse an `edition2024` manifest in the locked tree, 1.88 fails on `uuid 1.28.0`, and `cargo +1.89 check --locked --all-targets` passes. `Cargo.toml` now states `rust-version = "1.89"`, and `flake.nix` pins the same toolchain with `doCheck = true`. Nix is not installed here, so the flake build and its test run are unverified; the offline suite only binds loopback sockets, which the Nix sandbox allows.
