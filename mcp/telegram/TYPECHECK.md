# Python Diagnostic Triage

[Issue 52](https://github.com/Unique-Divine/jiyuu/issues/52) recorded findings
from ty 0.0.100 after the checker integration. The historical baseline remains
unchanged. These are diagnostic counts, not counts of confirmed runtime bugs.

| Project | Historical Errors | Historical Warnings | After Repairs |
| --- | ---: | ---: | ---: |
| Telegram MCP | 269 | 7 | 0 errors, 0 warnings |
| tg-sai Python generator | 8 | 6 | 0 errors, 0 warnings |

The baseline source is `ccd4b798bea76a73a89f41167eb8b44964b0df8b`, integrated
through [PR 53](https://github.com/Unique-Divine/jiyuu/pull/53). The locked checker
remains ty 0.0.100. Telethon remains 1.42.0 in the MCP and 1.44.0 in the standalone
generator. Both projects target Python 3.10; ordinary CI execution uses 3.13.
Frozen environments were also checked and tested under Python 3.10.19.

## Classification and Repairs

| Category | Source Evidence | Repair and Regression Evidence |
| --- | --- | --- |
| Incomplete annotations | Optional account, date, caption, and filter parameters defaulted to `None` with nonnullable annotations. Request error payloads returned bare dictionaries. | Nullable annotations and explicit payload types. `test_tool_schemas.py` checks registered schemas, required identifiers, and pagination defaults. Tool names, required inputs, and default values remain unchanged. Nullable input schemas intentionally accept explicit null. |
| Real timeout defect | `run_limited` caught operation-owned `TimeoutError`, including unbounded writes, as a policy deadline. | An owned child task separates policy expiration from operation errors. `test_request_policy.py` covers cancellation cleanup, cleanup failures, the completed-child race, and no cooldown recording or retry for cancellation. |
| Dependency typing limits | `telethon.sync` changes async methods at runtime. `get_messages` combines scalar and collection return variants in one annotation. Optional proxy imports and package metadata also had mismatched static contracts. | Explicit async authentication, small consumed-behavior protocols, supported metadata access, an optional dependency probe, and checked message result helpers. `test_message_results.py` retains remote totals and distinguishes missing IDs and scalar lookup errors. |
| Real RPC contract defects | Undefined error names, nonexistent convenience methods, dictionaries instead of generated contact/settings objects, unresolved peer variants, and marked IDs passed to basic-chat RPCs. | Use existing pinned Telethon methods and generated input objects. `test_rpc_contracts.py` resolves and serializes actual request classes without networking. Unsupported fallback write retries were removed. |
| Test-fake mismatches | Fakes used concrete client annotations, fake event-loop markers, invalid chat photos, and unchecked optional helper results. | Structural client protocols, actual async fake methods, valid SDK constructors, and assertions that narrow optional results. Both generator suites exercise disconnect on authentication failure. |
| Real GIF contract defects | The old search request and convenience fallbacks did not exist, and `send_file` cannot send a bare document ID. | Inline search returns document-backed IDs. A bounded per-client metadata store supplies complete documents to sends. `test_gif_documents.py` covers isolation, expiry, eviction, process reset, and no replay of writes. |

The saved anonymized MCP envelopes cover successful `get_me` and `get_messages`
outputs. Their live source revision is unverified. The replay tests preserve
that provenance and compare outputs from real SDK message fixtures. Synthetic
RPC fixtures separately test Telethon error mapping. Neither fixture set proves
that every Telegram response variant or server-side rate-limit scope was observed.

## Blocking Checks

Run `just python-typecheck` from the repository root to check both projects,
including tests and shared reporting helpers. Both strict commands fail on
warnings as well as errors. No source exclusions, project-wide `Any` replacements,
or diagnostic suppressions were added to reach zero.

CI calls the diagnostic report recipes with `enforce`. They write the native
output, structured report, and summary before returning failure for findings.
The default `report` mode still permits investigation without blocking on
checker exit 1. Setup failures, invalid configuration, checker crashes, and
unexpected exits remain failures in either mode. Reporter tests include a real
warning-only fixture to verify the blocking decision.
