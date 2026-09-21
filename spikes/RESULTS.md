# E0 results

Go/no-go spikes for Snail's GPUI foundations (plan.md §3, E0). Each spike appends its own section
below with the machine it ran on, the exact pin it tested, and a verdict.

Pin under test: `gpui-kit =0.6.4`, `gpui-pre =0.3.5`, `gpui-pre-platform =0.3.5` (newest pair at E0
start; plan.md §1.1 says freeze only after E0.2 passes).

## Test machines

| Machine | Details |
|---|---|
| Mac | Apple Silicon, macOS 26.5 (Xcode 26.5), Rust 1.97.1 |
| Linux | _pending E0.11_ |
| Windows | _pending E0.11_ |

---

## E0.1 — workspace scaffold

**Verdict: PASS on macOS.**

```sh
cargo build --workspace      # Finished dev profile in 4m36s
./target/debug/snail         # window opened, alive >6s, SIGTERM'd by the smoke test
```

- Pin `gpui-kit =0.6.4` / `gpui-pre =0.3.5` / `gpui-pre-platform =0.3.5` **resolves and builds** on
  Rust 1.97.1 (stable, aarch64-apple-darwin), Xcode 26.5. This is the newer pair plan.md §1.1
  anticipated; freezing it is contingent on E0.2.
- `\.with_assets(gpui_kit::assets::Assets)`, `WindowDecorations::Client`, `Root` as the shell parent,
  1240×820 window with a 900×600 minimum: all compile and open.
- Startup log is clean: two benign duplicate-font warnings from `gpui_macos::text_system`, and
  "system notifications disabled: not running from an app bundle" (expected for a bare binary;
  E18.1's `.app` bundle is where that resolves).
- One future-incompat warning from `block v0.1.6`, a transitive dep. Not ours; noted.
- No `rust-toolchain.toml` yet: stable 1.97.1 is what built. If a later `gpui-pre` needs 1.98.1
  (Zed's pin, plan.md §1.1) we add the file then.

Machine: Apple Silicon, macOS 26.5, Xcode 26.5, Rust 1.97.1.

<!-- filled in below once the build and window smoke test complete -->

---

## E0.2 — gpui-kit `Input`/`Textarea`

**Builds and renders; interactive/IME verdict pending a human at the keyboard.**

```sh
cd spikes/e0.2 && cargo build && cargo run   # compose window opened, alive >6s, no panic
```

API surface recorded from the 0.6.4 source (this supersedes what the plan assumed for 0.6.1):

- `InputState = InputBaseState<InputMode>`, `TextareaState = InputBaseState<TextareaMode>`, both
  `::new(window, cx)`; builders `.placeholder`, `.default_value`, `.set_value`, `.value`, `.text`,
  `.focus(window, cx)`, `.set_auto_grow(min, max, cx)` (multi-line only).
- `Input::new(&Entity<InputState>)`, `Textarea::new(&Entity<TextareaState>)`, `.tab_index(isize)`.
- **`InputEvent` is `Change | PressEnter { secondary, shift } | Focus | Blur`** — thin.
- Event subscriptions are on the **state** entity, and the returned `Subscription` must be retained
  or the feed is cancelled (caught in the spike, worth knowing in E7).
- gpui-kit 0.6.4 ships **`TextView::html(...)`** (`component::text::{html, markdown, TextView}`),
  backed by `gpui-base` — direct input for E6.13b.

Run `spikes/e0.2/CHECKLIST.md` for the tab order, clipboard/undo, wrap and CJK-IME pass on macOS,
Linux and Windows. **The go/no-go stands until that is filled in.**

**Interactive result, macOS, 2026-09-21 (owner):** select-all / copy / paste / cut / undo / redo
**work**. The remaining go/no-go question is CJK IME composition, plus tab order and soft wrap.

---

## E0.5 — app identity and paths

**PASS.** `snail-core::paths`:
- `Paths::resolve()` → config/cache/log dirs, all suffixed `dev.snail.app` by default.
- `SNAIL_CONFIG_DIR` / `SNAIL_CACHE_DIR` / `SNAIL_LOG_DIR` overrides, injectable for tests so no
  test races on process globals.
- Logs default under the cache dir so they are never iCloud-backed-up; cache is separate from config.
- `init_logging` writes a size-rotating file (5 MB → `snail.log.1`).
- 7 unit tests, including rotation and override handling. `cargo test -p snail-core` green.

## E0.7 — startup timeline

**PASS (instrumented).** `snail::startup::{begin, mark, snapshot, summary}` logs
`startup <name> @ <ms>ms`. Wired into `main.rs`: `paths_and_logging`, `app_launched`, `gpui_init`,
`window_opened`, `shell_built`, `root_built`, `first_frame`. The `fonts`/`services`/`models` marks
join as E1/E2 create them. 2 unit tests.

---

## E0.4 — both accounts authenticate and pull one page

**Verdict: PASS — both accounts. Several plan claims are superseded by the live results below.**

Run: `cd spikes/e0.4 && cargo run -- google|corpus|icloud|caldav`.

### Google — PASS

- Loopback OAuth + PKCE worked first try (Desktop client, `code_challenge_method=S256` explicit,
  `127.0.0.1`, state checked). Four scopes granted: `gmail.modify`, `gmail.labels`, `calendar`,
  `calendar.events`. Refresh token stored; a second run refreshed without consent.
- Gmail profile: **669,873 messages / 456,442 threads**. This is a decisive input for §6.5 and open
  question 3 — at `messages.get` = 20 units and 300 fetches/min/user, a full backfill is a
  **~37-hour** wall-clock floor, not the 167 minutes the plan used for 50k. More threads than a
  `threads.get` backfill can hide in pagination; backfill depth needs a decision.
- 50 INBOX headers fetched; `format=metadata` works.
- Calendar: `calendarList` returned **14 calendars** (owner/reader mix); `events.list` on primary
  returned 50 on the first page.
- One real bug found and fixed: requesting `Accept-Encoding: gzip` without enabling reqwest's
  `gzip` feature leaves the body compressed and JSON parsing fails at column 1. Enable the feature
  (E4.9's point stands, but it is not just a header).

### E0.3 corpus — 24 raw messages dumped

`cargo run -- corpus` wrote 24 `.eml` files (2.8 MB) across newsletter / github / calendar-invite /
quoted-chain / attachment / plain / receipt / notification. Gmail's `raw` is sometimes padded
base64url, so the decoder must be lenient. These are the E0.3 renderer corpus and live in gitignored
`spikes/corpus/`.

### iCloud IMAP — PASS

Logged in as the **full address** on the first attempt (the local-part fallback also exists).
Findings, all captured fresh:

- **Pre-auth** capabilities (2026): `XAPPLEPUSHSERVICE IMAP4 IMAP4rev1 SASL-IR AUTH=ATOKEN AUTH=PLAIN
  AUTH=ATOKEN2 AUTH=XOAUTH2`. No IDLE/CONDSTORE/QRESYNC before login — §6.7 confirmed, and
  `AUTH=XOAUTH2`/`AUTH=ATOKEN2` are new since the 2020 dump.
- **Post-auth**: `CONDSTORE ENABLE QRESYNC IDLE UIDPLUS NAMESPACE ESEARCH SORT THREAD QUOTA WITHIN
  LIST-STATUS ID …` plus `XAPPLEPUSHSERVICE`, `XAPPLELITERAL`, `X-APPLE-REMOTE-LINKS`.
- **`ENABLE CONDSTORE`: OK.**
- **`SELECT INBOX (CONDSTORE)`: OK (6739 messages).** This **contradicts §6.7 / E4.13(a)**, which
  said iCloud rejects that form with `BAD Unexpected extra arguments`. As of 2026-09-21 the
  parameter form works, so E4.13 can use the simpler path.
  (`async-imap`'s `select_condstore` sends exactly `SELECT "INBOX" (CONDSTORE)`.)
- **Folders**: `Archive`, `Deleted Messages` **`[\Trash]`**, `Junk`, `Notes`, `Sent Messages`
  **`[\Sent]`**, `INBOX [\NoInferiors]`, `Drafts`. This **contradicts E4.18**: iCloud *does* now
  advertise SPECIAL-USE on `Sent` and `Trash`. Names still differ from the plan's guess
  (`Sent Messages` / `Deleted Messages` are correct), and `Archive`/`Junk`/`Notes`/`Drafts` carry no
  attribute, so name-mapping is still needed for those — but flag-based discovery now covers two of
  the five.
- INBOX: 6739 messages, UIDVALIDITY 1502348654, UIDNEXT 6741. 50 headers fetched; RFC 2047
  encoded-words need decoding downstream (mail-parser's job).

### iCloud CalDAV — PASS

- `GET /.well-known/caldav` returns **400**; a **`PROPFIND`** on it returns 207 with
  `current-user-principal`. §6.8's "301 redirect" framing is wrong for a GET; the well-known is a
  DAV resource, not a redirect target.
- Principal: `/285963941/principal/`. `calendar-home-set` is a **CalDAV** property
  (`urn:ietf:params:xml:ns:caldav`), not `DAV:` — asking in the wrong namespace silently 404s it.
- Home resolves to the **sharded host**: `https://p146-caldav.icloud.com:443/285963941/calendars/`
  — §6.8's sharding confirmed. Responses use a default `DAV:` namespace (`<response xmlns="DAV:">`),
  not prefixes; a naive prefixed parser matches nothing.
- 10 collections, including `inbox/`, `outbox/` and `notification/` — i.e. **server-side scheduling
  (RFC 6638) endpoints exist**, which E13.8's iCloud RSVP probe needs.
- **RFC 6578 `sync-collection` is advertised: YES** — the plan's "highest-value ten-minute test"
  answers in favour of the incremental path (E11.13).
- `calendar-query` REPORT on a real calendar → 207.

### Still open in E0.4

- The **day-8 refresh-token check** (E0.6): first consent was 2026-09-21, so re-check on or after
  2026-09-29.
- iCloud sent-copy filing, the SMTP quota status code, UIDVALIDITY stability and the connection
  limit are not yet probed (they need a send, not a read).
- `getctag` came back empty on the Depth-1 PROPFIND; `supported-report-set` did not. Worth a
  follow-up request (iCloud may want `cs:getctag` explicitly).

---

## E0.3 — HTML renderer over the owner's worst messages

**Corpus: 24 real messages, 2.8 MB** (gitignored `spikes/corpus/`), dumped from Gmail
`format=raw` by `spikes/e0.4 corpus`: newsletter / github / calendar-invite / quoted-chain /
attachment / plain / receipt / notification. Plus **one committed synthetic fixture**
(`spikes/e0.3/fixtures/format-flowed.eml`) because the `plain` query returned HTML-bodied mail and
so `format=flowed` was never exercised by real mail.

**Analyze (`cargo run --bin e0-3-analyze`) — the failure taxonomy:**

| Measure | Result |
|---|---|
| Real HTML bodies | **24 / 24** |
| Contain `<table>` | **23 / 24** |
| Nested ≥2 deep | **21 / 24** |
| Nested ≥3 deep | **21 / 24**; deepest **13** |
| Inline `cid:` images | 4 / 24 |
| Use a deliberately-unsupported property (float/position/flex/grid/transform/@media) | **23 / 24** |
| `format=flowed` plain text | 1 (the synthetic fixture) |
| MIME parse | avg **0.41 ms**, max **2.10 ms** |
| Sanitize (ammonia) | avg **1.32 ms**, max **3.69 ms** |

**Body-selection sharp edge found while adding the fixture (input for E6.1):**
`Message::body_html(pos)` **converts** `text/plain` to HTML, and `html_body_count()`/`html_bodies()`
are **not** a reliable "does this message have real HTML" signal — for an alternative (or a lone
part) mail-parser **copies the part across both lists**. Selecting on the aggregate lists therefore
misclassifies plain mail as HTML. The correct selector is the part's MIME type:
`message.html_bodies().any(|part| part.is_text_html())`, then `text_bodies()` / `is_text()`.
Recorded in the analyzer and worth carrying into E6.1.

**Read:** parse + sanitize together are ~1.7 ms average and never exceed 5.8 ms, so the CPU cost
before layout is comfortably inside a frame. The hard part is exactly what the plan predicted:
**tables, and nested tables** — E6.6 is the epic, not an edge case. The 4 `cid:` messages and the
one newsletter with 274 remote URLs exercise E6.2/E6.8.

Worst offenders by unsupported constructs: `receipt-2` (43), `quoted-chain-2`/`receipt-1` (27,
67 tables each at depth 9), the two calendar invites (15).

**E6.13b — `TextView::html`: present, but NOT a shortcut to the reading pane.**
gpui-kit 0.6.4 ships it (`component::text::TextView::html(id, text)`, `.selectable()`,
`.scrollable()`, `.table_actions()`, `.on_link_click()`), and it renders the corpus — but the owner
reports **“they render but there is no formatting.”** Reading `gpui-base`'s
`text/format/html.rs` explains why:

- It is a **Markdown-grade HTML subset**. It recognizes text/headings/lists/`br`/`hr`/`pre`/`code`,
  `b`/`strong`/`i`/`em`/`u`, `a`, `img`, `blockquote`, `mark`, and `table`/`thead`/`tbody`/`td`/`th`.
- From an inline `style` attribute it honors **only `color`, `background-color`, `width`,
  `height`**. There is no `font-family`, `font-size`, `font-weight` (outside the tag form),
  `text-align`, `margin`, `padding`, `border`, `line-height`, `display` or `vertical-align`.
- `<style>` and `<script>` blocks are dropped outright.

So it cannot meet E6.4's subset, which is exactly what real mail uses. **E6 still needs its own
layout (6.5–6.7).** `TextView` stays valuable as **6.13's always-renders-something fallback**, and
may be enough for simple messages, but it is not the reading pane.

A second, independent cause: **ammonia's default strips the `style` attribute and `<style>`
content**, so a sanitize-then-TextView pipeline loses even the `color`/`width` that `TextView` could
have shown. E6.2/E6.3 must parse the supported style subset *before* (or instead of) ammonia's
default attribute handling.

To separate the two layers, `e0-3-render --raw` renders the original `.eml` bodies unsanitized.

**Finding while rendering:** `TextView::html` does **not** block remote content — it tried to fetch
remote images itself (`Failed to load asset ... "http://pixel.watch/..."`, `No HttpClient
available`) for the newsletters that carry tracking pixels and hosted images. Nothing was fetched
(no HTTP client is installed), but this proves E6.2's rule is load-bearing: **remote URLs must be
rewritten to a blocked placeholder before the HTML reaches any renderer**, including `TextView`.

**Prototype (E6.5 seed): a minimal layout in `snail-ui`.** `crates/snail-ui/src/html.rs` (DOM +
style subset + `resolve_style`) and `crates/snail-ui/src/layout.rs` (block stacking, greedy inline
wrapping with alignment, table columns with padding/borders) produce a flat display list from a
`TextMeasure` trait. 9 unit tests run against a fake metrics impl, with no GPUI. The GPUI bin
`e0-3-layout` parses MIME → sanitizes (keeping the supported `style` subset) → html5ever →
`snail-ui` DOM → layout → fragments.

A headless `--dump` mode (layout against fake metrics, no window) caught a real bug immediately:
`thead`/`tbody`/`tfoot` were being classified as `Display::TableRow`, so `collect_rows` treated the
section as a row, found no cells, and **silently dropped every table's content** (the USPS message
laid out as 38 fragments / 70px with two visible words). With sections as pass-through, the same
message is 506 fragments / 3022px with a 32px centred heading and 16–22px body — the value of
splitting layout into a GPUI-free crate, demonstrated on day one.

Still approximate: `colspan`/`rowspan`, deeply nested tables, real font metrics, and `<style>`/
`class` resolution (E6.3) are not done.

**Go/no-go:** not yet decided. Parse/sanitize pass comfortably. Readability does **not** pass with
`TextView::html`, which makes E6.5–E6.7 necessary rather than optional — a scope confirmation, not a
failure of the spike.

---

## E0.4 — both accounts authenticate and pull one page

_pending — credentials are a hard input (see the E0 ask list)_
