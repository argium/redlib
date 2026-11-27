# Redlib Audit Remediation

This README indexes the project workspace. Keep it current as artifacts are created, approved, run, or superseded. The artifacts contain the source material; this file provides navigation and current status.

| Field | Value |
| --- | --- |
| Status | In progress |
| Owner | Argi |
| Created | 2026-11-28 |
| Updated | 2026-11-28 |

## Summary

Remediation of the 55 confirmed bugs and medium-to-high value improvements in [research.md](./research.md), organised into 7 vertical slices: OAuth/client reliability, security/DoS hardening, server performance/observability, domain/feed correctness, frontend JS/accessibility, theme contrast/typography/UX, and container/CI hygiene. Every milestone needs verification evidence and an offline regression unit test.

## Workflow status

Engineering-led project without a separate `spec.md`; acceptance criteria live in `plan.md` milestones. The plan and its defaults were approved on 2026-11-28. Slice 1 is accepted on `fix/oauth-client-reliability` (see its Acceptance Evidence). Slice 2 is implemented on `fix/security-and-input-hardening`; its evidence is in `tasks-slice-2.md`.

## Artifacts

| Artifact | Purpose | Status |
| --- | --- | --- |
| [research.md](./research.md) | Severity-ranked audit findings (#1–#55) | Complete |
| [intent.md](./intent.md) | Request history and agreed decisions | Agreed |
| [plan.md](./plan.md) | 7 vertical slices, milestones, and acceptance checks | Ready |
| [tasks-slice-1.md](./tasks-slice-1.md) | Slice 1 executable tasks | Complete |
| [tasks-slice-2.md](./tasks-slice-2.md) | Slice 2 executable tasks | Complete |

## Next action

Open the Slice 2 draft PR stacked on Slice 1, then compile Slice 3 into `tasks-slice-3.md` on `perf/server-and-observability`.

## References

- Upstream: [redlib-org/redlib](https://github.com/redlib-org/redlib)
