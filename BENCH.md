# Benchmarks

Recorded results, with the machine, the fixture and the command. Budgets live in plan.md §4.

## Machines

| Machine | Details |
|---|---|
| Mac | Apple Silicon, macOS 26.5, Rust 1.97.1 |

## E2.9 / E0.9 — store, 2026-09-21

Fixture: `snail --generate-fixture` — 2 accounts, 16 mailboxes, **200,000 messages**, 20,000 events,
generated in ~3.0 s. On disk: **96 MB** (DB + cache).

Command: `SNAIL_CONFIG_DIR=… SNAIL_CACHE_DIR=… snail --bench-store`

| Metric | Result | Budget (§4) |
|---|---|---|
| Cold open | **2.0 ms** | < 400 ms startup, of which this is one part |
| Mailbox page query (100 rows) p50 / p99 | **0.11 / 3.80 ms** | not gated until the list budget (E5.12) |
| Thread assembly | **0.58 ms** | — |
| Unread counts, 8 mailboxes | **0.23 ms** | — |

**Finding:** the first `unread_count` implementation used `count(*)` and took **138 ms** for eight
mailboxes on the fixture. The sidebar reads the stored `mailbox.unread` counter instead; the
`count(*)` path is kept as `recount_mailbox` for rebuilds and full resyncs. The list budget
(`< 2 ms` draw p50) is a paint-time budget and lands in E5.12; these are query times.

## E4 — real-mail backfill, 2026-09-21

`snail --backfill-gmail --limit 100` (credentials from the environment, nothing printed) against the
real account: **669,881 messages**, the last 30 days enumerated and **100 inserted**, head captured
before enumeration per E4.2. **9.5 MB** of raw MIME in the content-addressed cache. Subsequent
`--bench-store` on that store: cold open 1.3 ms, page 0.01/0.02 ms, thread 0.03 ms, unread counts
0.03 ms.

**Gap found and stated:** the raw fetch carries no labels, so backfilled messages have no mailbox
yet; filing by label is the sync loop's job (E4.7/E8.6).
