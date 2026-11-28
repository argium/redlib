# Redlib Audit Remediation — Intent

| Field | Value |
| --- | --- |
| Status | Agreed |
| Author | Argi |
| Created | 2026-11-28 |

## Overview

Address the confirmed bugs and medium-to-high value improvements identified in the static audit of Redlib (`0.40.0`) recorded in [research.md](./research.md). The goal is a sequenced, reviewable implementation plan covering OAuth and upstream reliability, security and access control, performance, domain and feed correctness, frontend/UX/accessibility/typography, and container/CI/testing hygiene.

## Open Questions

- None. All five planning questions were answered on 2026-11-28 (see Clarifications and Decisions).

## Original Request

Session prompts (2026-11-28):

> Find bugs that need to be fixed. critical and important only - no nits.
> Suggest improvements to the end-user experience, performance, reliability, development, and debugging. Medium or high value only. No low value. No nits.
>
> READ ONLY. make a list-> IMPROVEMENTS.md
>
> fanout.

> also include one agent dedicated to designing interfaces, UX and typography

> remove anything that is too speculative

> print and rank in order of sevierty

> yes *(re-ordering `IMPROVEMENTS.md` in severity order)*

> `<skill name="to-plan" ...>` *(invoked on this session / `IMPROVEMENTS.md`)*

> for each, require verification and a regression unit test.
>
> `<skill name="to-tasks" ...>`
>
> move documents to the folders/paths prescribed by skills (ignore ".plans")
>
> approve and start work

## Clarifications and Decisions

- **2026-11-28 (session, agreed):** Added a dedicated UI/UX, accessibility, and typography audit pass.
- **2026-11-28 (session, agreed):** Removed speculative items (untested `seccomp-redlib.json` syscall list, `token_daemon` `expires_in < 120` underflow, unverified `v.redd.it` non-`DASH_` regex fallback, and unverified Reddit non-hex flair colour injection).
- **2026-11-28 (session, agreed):** Ranked the final 55 confirmed items by severity in [research.md](./research.md).
- **2026-11-28 (plan path override, superseded):** Planning artifacts were first placed under `.plans/` per the user's global `AGENTS.md`.
- **2026-11-28 (session, agreed):** Moved all artifacts to the skill-prescribed `projects/2026-11-28-redlib-audit-remediation/` folder, superseding the `.plans/` placement. The audit list (`IMPROVEMENTS.md`) moved to [research.md](./research.md) as the project's supporting research.
- **2026-11-28 (session, agreed):** Approved the plan and its proposed defaults: all 55 items are in scope (medium-value items included); delivery is the proposed 7-layer stacked PR; #3 escapes `pulldown_cmark` raw HTML events without a new crate; #8 fails startup with a non-zero exit when an existing `redlib.toml` cannot be read or parsed; #28 wires `REDLIB_DEFAULT_AUTOPLAY_VIDEOS` into `Config`.
- **2026-11-28 (session, agreed):** Every milestone requires verification evidence and a regression unit test that fails before the fix and passes after it.
- **2026-11-28 (session, agreed):** Commit without signing when signing is unavailable ("skip commit sign when applicable").
- **2026-11-28 (orchestrator, reversible):** MSRV recorded as Rust 1.89, the oldest toolchain that builds the locked dependencies; `flake.nix` pins it.
- **2026-11-28 (session, agreed):** Leave `flake.nix` untouched (Nix is not used here).
- **2026-11-28 (session, agreed):** Drop four speculative items from the PR: #1 (stuck rollover flag; no realistic trigger, so the PR no longer lists it), #9 (redirect hop cap; needs Reddit to redirect without `Location`), #14 (restore body when compression fails; in-memory gzip/brotli does not fail in practice) and #39 (log render errors; Askama render failures are compile-time checked). The code for #9, #14 and #39 was reverted. The `RolloverGuard` and the spawned `401` refresh stay, because #2's refresh cooldown needs them.
