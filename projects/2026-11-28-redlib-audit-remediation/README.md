# Redlib Audit Remediation

This README indexes the project workspace. Keep it current as artifacts are created, approved, run, or superseded. The artifacts contain the source material; this file provides navigation and current status.

| Field | Value |
| --- | --- |
| Status | In review |
| Owner | Argi |
| Created | 2026-11-28 |
| Updated | 2026-11-28 |

## Summary

Remediation of the 55 confirmed bugs and medium-to-high value improvements in [research.md](./research.md), organised into 7 vertical slices: OAuth/client reliability, security/DoS hardening, server performance/observability, domain/feed correctness, frontend JS/accessibility, theme contrast/typography/UX, and container/CI hygiene. Every milestone needs verification evidence and an offline regression unit test.

## Workflow status

Engineering-led project without a separate `spec.md`; acceptance criteria live in `plan.md` milestones. The plan and its defaults were approved on 2026-11-28. Slice 1 is accepted on `fix/oauth-client-reliability` (see its Acceptance Evidence). Slice 2 is implemented on `fix/security-and-input-hardening`; its evidence is in `tasks-slice-2.md`. Slice 3 is implemented on `perf/server-and-observability`; its evidence is in `tasks-slice-3.md`. Slice 4 is implemented on `fix/domain-feeds-and-search`; its evidence is in `tasks-slice-4.md`. Slice 5 is implemented on `fix/frontend-a11y-and-js`; its evidence is in `tasks-slice-5.md`. Slice 6 is implemented on `feat/ux-themes-and-typography`; its evidence is in `tasks-slice-6.md`.

## Artifacts

| Artifact | Purpose | Status |
| --- | --- | --- |
| [research.md](./research.md) | Severity-ranked audit findings (#1–#55) | Complete |
| [intent.md](./intent.md) | Request history and agreed decisions | Agreed |
| [plan.md](./plan.md) | 7 vertical slices, milestones, and acceptance checks | Implemented |
| [tasks-slice-1.md](./tasks-slice-1.md) | Slice 1 executable tasks | Complete |
| [tasks-slice-2.md](./tasks-slice-2.md) | Slice 2 executable tasks | Complete |
| [tasks-slice-3.md](./tasks-slice-3.md) | Slice 3 executable tasks | Complete |
| [tasks-slice-4.md](./tasks-slice-4.md) | Slice 4 executable tasks | Complete |
| [tasks-slice-5.md](./tasks-slice-5.md) | Slice 5 executable tasks | Complete |
| [tasks-slice-6.md](./tasks-slice-6.md) | Slice 6 executable tasks | Complete |
| [tasks-slice-7.md](./tasks-slice-7.md) | Slice 7 executable tasks | Complete |

## Next action

All 7 slices are implemented and open as stacked draft PRs (argium/redlib#1–#7, each based on the previous branch). Next: get CI running on the fork, review and merge bottom-up from #1, then plan a `cargo update` to clear the `cargo deny` advisories and make that job blocking.

## References

- Upstream: [redlib-org/redlib](https://github.com/redlib-org/redlib)

## Dropped after review

Items #1, #9, #14 and #39 of [research.md](./research.md) were dropped as speculative; the task and plan files still describe them as originally planned. Their code was reverted, except the `RolloverGuard` and the spawned `401` refresh from #1, which the #2 refresh cooldown depends on. For #53 only the log-level change was dropped: the token prefix, device ID and spoofed-header lines log at `info` again because admins use them to debug, while the auth-error redaction and the `ResponseExt::cookies()` fix stay. See [intent.md](./intent.md).
