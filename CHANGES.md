# Changes

Audit remediation for Redlib `0.40.0`: 51 items, grouped by type and ranked by criticality within each group. Numbers are the audit item numbers in [`projects/2026-11-28-redlib-audit-remediation/research.md`](projects/2026-11-28-redlib-audit-remediation/research.md).

**Criticality** is a judgement of impact times likelihood: **High** means real impact that is easy or common to hit, **Medium** means real but conditional or contained, **Low** means rare, cosmetic or hardening only.

**Verified** says how each item was checked:

- **live:** observed against real Reddit or a real running instance.
- **local:** observed running the real binary, container or bundled file on a development machine, but not against live Reddit.
- **synthetic:** shown with a fixture or test that failed before the fix and passes now.
- **measured:** measured in a browser or by computation on rendered fixtures.
- **not verified:** read only, or never run.

Items #1, #9, #14 and #39 were dropped as speculative and are not listed.

## Security (10)

| Rank | # | Issue | Criticality | Verified | Why |
|---|---|---|---|---|---|
| 1 | 3 | HTML injection in code-fence posts | High | live | A real post on an unpatched instance renders raw `<h2>`/`<hr>`. The redirect payload is only on test pages. |
| 2 | 7 | SFW-only mode bypass | High | synthetic | Red tests. The handler wiring is read only. |
| 3 | 5 | Open redirects | High | synthetic | Red tests. |
| 4 | 4 | Memory exhaustion (cache, POST size, deflate bomb) | Medium | synthetic | Red tests, including a deflate bomb. |
| 5 | 6 | `IPV4_ONLY=false` overrides `--address` | Medium | synthetic | Red tests. |
| 6 | 8 | Invalid `redlib.toml` silently ignored | Medium | local | The built binary exits 1 on a bad file. |
| 7 | 11 | Container workflow publishes after build failure | Medium | not verified | The workflow has never run on GitHub. |
| 8 | 33 | Short-link IDs not validated | Low | synthetic | Red test. |
| 9 | 45 | No HTTP header-read timeout | Low | synthetic | Loopback slow-header test. |
| 10 | 53 | Auth error log can include the access token; `cookies()` reads the wrong header | Low | synthetic | Redaction test and `cookies()` test. |

## Bug fixes (27)

| Rank | # | Issue | Criticality | Verified | Why |
|---|---|---|---|---|---|
| 1 | 2 | Auth outage during refresh exits the process | High | synthetic | Fake auth backend. |
| 2 | 12 | Dockerfile, compose and ubuntu healthcheck broken | High | local | Podman builds failed before the fix. Each image reached healthy after. |
| 3 | 15 | HLS recovery broken (`ErrorType` typo, endless retry) | High | local | The bundled `hls.js` has `ErrorTypes` only. Retry logic tested with fakes. |
| 4 | 38 | Misleading Reddit error messages | Medium | live | The real 403 now shows the "blocked" message. |
| 5 | 17 | Search breaks on `&`, `#`, `+`, `%` | Medium | synthetic | Red tests. |
| 6 | 48 | Unencoded links, invalid and duplicate markup | Medium | synthetic | Red tests and a markup checker. |
| 7 | 13 | Server errors exit `0` | Medium | local | Built binary exit codes. |
| 8 | 10 | GitHub unreachable causes panics | Medium | synthetic | Loopback tests. |
| 9 | 29 | No outbound HTTP timeouts | Medium | synthetic | Loopback tests. |
| 10 | 21 | Keyboard accessibility gaps | Medium | measured | Chromium on rendered fixtures. |
| 11 | 22 | Low contrast across themes | Medium | measured | Computed contrast ratios. |
| 12 | 23 | Layout breakage (rows, popup, navbar) | Medium | measured | Chromium on rendered fixtures. |
| 13 | 20 | Wiki shortcut drops the page | Medium | synthetic | Red test. |
| 14 | 18 | Comment search matches HTML | Medium | synthetic | Red test. |
| 15 | 25 | OpenSearch points at localhost | Medium | local | The local server's `/opensearch.xml` was checked. |
| 16 | 28 | Config drift (`app.json`, autoplay default) | Medium | synthetic | Key-name test. |
| 17 | 16 | RSS gating, escaping and `<link>` | Medium | synthetic | Fixture XML read back with the `rss` crate. |
| 18 | 26 | Update checker `TypeError` | Low | synthetic | Node test with DOM fakes. |
| 19 | 24 | Typography (px sizes, line-height, font loading) | Low | measured | Chromium. The font-loading part is read only. |
| 20 | 51 | `color-scheme`, reduced motion, forced colours | Low | measured | Emulated in Chromium. |
| 21 | 43 | Relative `og:` URLs | Low | synthetic | Red tests. |
| 22 | 52 | `highlighted.js` missing on image/video threads | Low | synthetic | Render test. |
| 23 | 27 | `/info.txt` mismatched values | Low | synthetic | Red test. |
| 24 | 50 | Invalid gallery items | Low | synthetic | Red test. |
| 25 | 19 | `rewrite_emotes` mispairs emotes | Low | synthetic | Hand-written fixture only. |
| 26 | 46 | Missing `Vary: Accept-Encoding` | Low | synthetic | Header test. |
| 27 | 32 | No panic boundary | Low | synthetic | Panicking test route. |

## Features and improvements (14)

| Rank | # | Issue | Criticality | Verified | Why |
|---|---|---|---|---|---|
| 1 | 34 | Request logs, `/healthz`, `--healthcheck` | Medium | local | Real binary tests, and Podman containers reached healthy. |
| 2 | 37 | CI coverage, single release build, dependency pins | Medium | local | `actionlint`, Rust 1.89 check, lockfile unchanged. The CI itself never ran. |
| 3 | 36 | Separate live-Reddit tests, drop 10 retries | Medium | local | The offline suite runs. |
| 4 | 42 | Error, empty, gallery and copy feedback | Medium | synthetic | Render tests and JS tests. |
| 5 | 41 | Mobile layout, touch targets, wide-mode width | Medium | measured | Chromium at several widths. |
| 6 | 35 | JS caching, `hls.min.js` only when needed | Medium | synthetic | Route and header tests. |
| 7 | 30 | Brotli quality 11 off the async threads | Low | not verified | Never benchmarked. A test checks it runs off the executor. |
| 8 | 40 | Comment thread hierarchy and indent cap | Low | measured | Chromium. |
| 9 | 47 | Coalesce duplicate fetches, cheaper lookups | Low | synthetic | Counter test on a stand-in fetch. |
| 10 | 31 | `Preferences` built once per request | Low | not verified | Never benchmarked. |
| 11 | 44 | Skip link and landmarks | Low | measured | Chromium keyboard check. |
| 12 | 49 | Versioned settings export | Low | synthetic | Old and new export tests. |
| 13 | 55 | Docker context, devcontainer, `justfile` | Low | local | Podman builds and `just` ran. The devcontainer was not tested. |
| 14 | 54 | Monospace stack, labels, theme names, reset | Low | synthetic | Render tests. |

## Notes

- **Grouping:** #8 and #11 are under Security because they fail open on a security setting or weaken the supply chain. #29 and #32 are under Bug fixes, though they could also count as hardening. #49 is under Features because versioning is its main purpose, not the emoji panic fix.
- **#53:** it keeps only the auth-error redaction and the `cookies()` fix. The extra `info` logging of the token prefix and device IDs is unchanged, because admins use it to debug.
- **Live checks:** only #3 and #38 were seen against real Reddit or a real instance, because the development machine's connection is blocked by Reddit's API. #3 is live for the raw HTML passthrough in a real post and synthetic for the redirect payload.
- **Not verified:** #11, #30 and #31 are plainly unverified, and #37's CI workflow changes never ran on GitHub.
