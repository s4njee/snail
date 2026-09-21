# Snail — plan

A lightweight mail and calendar client for macOS, Linux and Windows, built in Rust on GPUI, to the
`design_handoff_email_client` and `design_handoff_almanac_calendar` specs.

The premise, from the mail handoff and restated as the project's governing constraint: **fewer
features.** No rules engine, no smart folders, no VIPs, no plugin system, no local-mbox import.
Thunderbird is the anti-goal. What is in scope is the loop you actually run all day — sync, read,
triage, reply, and see your week — done fast enough that opening it never feels like a decision.

**Requirements fixed with the owner (2026-09-20):**
- v1 mail is **read + triage + reply**, plus **PGP** (sign/verify/encrypt/decrypt).
- HTML email is rendered by a **custom Rust renderer inside GPUI** — no embedded webview.
- Accounts: **Gmail + Google Calendar** and **iCloud Mail + iCloud Calendar**.
- **Cross-platform is a hard requirement.** macOS is the daily driver and gets the polish budget;
  Linux and Windows stay green in CI and usable, and are not deferred to "later" as a port.
- This is a **personal tool**, not a product. That decision buys out an entire epic's worth of
  work (OAuth app verification, signed installers, onboarding, crash reporting) and is the reason
  E0.6 below can stay inside Google's unverified-app user cap.

---

## 1. Decisions

### 1.1 The GPUI dependency — settled, reusing Ferrite's answer

`~/projects/foobar2001` already paid for this investigation and measured the result on real
hardware (`GPUI.md` §2, `spikes/G0-RESULTS.md`, 2026-09-14). Snail adopts the same pin rather than
re-deriving it:

```toml
gpui-kit = { version = "=0.6.1", default-features = true }
# Named `gpui` because GPUI's `Action` derive generates `gpui::` paths.
gpui = { package = "gpui-pre", version = "=0.3.2" }
gpui-pre-platform = "=0.3.2"
```

The reasoning that produced it still holds: `gpui` 0.2.2 on crates.io is stale (2025-10-22); Zed
`main` split the platform code into unpublished crates; Zed does not support GPUI as a
general-purpose library and points people elsewhere. `gpui-kit` 0.6.1 (Longbridge, Apache-2.0)
republishes a Zed `main` snapshot as `gpui-pre` and adds the components this app needs most —
**Input and Textarea above all**, plus Menu/ContextMenu, Popover, Dialog, Tooltip, Kbd, VirtualList,
Resizable and JSON themes.

Exact (`=`) pins on the whole family are mandatory, not tidiness: every crate in the tree must
resolve to one GPUI source or the types do not match. Upgrades are deliberate, one bump at a time,
and 0.6 has already broken Input and Table once.

Two updates since Ferrite fixed its pin on 2026-09-14, to resolve in E0.1 rather than inherit
blindly. **gpui-kit is now 0.6.4 (2026-09-18)** on `gpui-pre` 0.3.5, and the project has been
renamed from `gpui-component` to **GPUI Kit**; it is extremely active, with same-day commits. And
note that `gpui-kit` depends on `gpui-pre` by **semver, not an exact pin** — so our own `=` pins on
`gpui-pre` are what actually stop cargo floating. Take the newest pair that builds and passes E0.2,
then freeze; do not split the difference between snapshots.

Also worth knowing before E0.1: Zed's own `rust-toolchain.toml` pins **Rust 1.98.1** with edition
2024, while this machine is on 1.97.1. Ferrite's 0.3.2 pin builds on 1.97.1; a newer `gpui-pre`
snapshot may not. And **macOS builds need full Xcode, not just the Command Line Tools**, because the
Metal shader compiler ships inside Xcode.

**Snail leans on gpui-kit's `Input`/`Textarea` far harder than Ferrite did** — Ferrite has three
inputs in the whole app; Snail has a compose window, a search field, an event editor with a dozen
fields, and account setup forms. That concentrates the upgrade risk in exactly the component the
vendor has already broken once. E0.2 spikes it before anything else is built on top.

### 1.2 Carried-over GPUI facts that are already paid for

These come out of Ferrite's G0 spikes and 21.7k lines of shipped GPUI code. They are stated here so
no story in this plan re-learns them:

| Fact | Consequence for Snail |
|---|---|
| `.with_assets(gpui_kit::assets::Assets)` is required, and is omitted from the upstream hello-world | Without it **every** gpui-kit icon silently paints nothing, including the Windows/Linux window controls. E1.1. |
| gpui-kit starts in `ThemeMode::Light` | Its chrome is unreadable on any other palette until the theme is mapped. Map the palette onto gpui-kit's `Theme` at startup, not around it. E1.4. |
| `TitleBar::window_options()` leaves `window_decorations` unset | Set `WindowDecorations::Client` yourself or wlroots compositors draw a second title bar above yours. E1.2. |
| GPUI has **no letter-spacing**; `TextStyle` has no tracking field and zed#16686 is closed "not planned" | Both handoffs use negative tracking on display type (-0.03em) and wide positive tracking on mono micro-labels (0.12em). Needs Ferrite's `tracked.rs` approach: shape the line once, position each glyph. Short non-editable labels only — never in a list row. E1.5. |
| Rows must be `w_full()` inside `uniform_list` | Or columns collapse. It silently corrupted Ferrite's first benchmark. E5.1. |
| Rust 2024 captures argument lifetimes in `impl IntoElement` returns | Row builders that clone strings need `-> impl IntoElement + use<>`. |
| `request_animation_frame` panics outside render | Continuous loops use `cx.notify()` + `cx.on_next_frame` called *from* render. Matters for the calendar now-line. E12.6. |
| GPUI panics if a hover style is set twice | Build hover closures conditionally; never chain two `.hover()`. |
| `[profile.dev.package."*"] opt-level = 2` | GPUI is unusable in a plain debug build. Day one. |
| Linux **requires Vulkan** | GPUI ignores `WGPU_BACKEND`; the GL path panics in the quads pipeline (zed#50996). A Linux box without a Vulkan driver cannot run Snail. Documented in BUILDING.md, detected at startup. |
| Linux file dialogs are portal-only (`ashpd`) | Needs `xdg-desktop-portal-gtk` or equivalent installed; show a real error if no portal answers. Attachments depend on this. |
| AccessKit **has landed** in GPUI (merged 2026-05-27) but is gated behind `ZED_EXPERIMENTAL_A11Y=1`, the tree is empty until components annotate themselves, and Windows screen readers are openly broken | Better than "no accessibility", still not usable. gpui-kit 0.6.1+ does expose roles/labels/values across its components, so using its widgets is the cheapest path to any a11y at all. See §7. |
| GPUI has **no `z-index`** — stacking is paint order | Escapes are `deferred(child).with_priority(n)` and `anchored()`. Needed for the event-editor scrim (E13.1), overlapping calendar blocks (E12.2), and absolutely-positioned boxes in the HTML renderer (E6.7). |
| Taffy on this snapshot **does** support CSS Grid (`grid()`, `grid_cols(n)`, `grid_rows(n)`, `GridPlacement::Line`) | The month view's 6×7 grid and the weekday strip are a direct fit — don't hand-roll them out of nested flex. E12.3. |
| `uniform_list` **requires uniform row heights** (it measures the first element and extrapolates); `list(ListState, …)` handles variable heights but ships no scrollbar and reports a wrong thumb until you scroll unless you set `with_uniform_item_height` | Decides list strategy everywhere: fixed-height message rows get `uniform_list`; the HTML document and agenda need `list` or normalized row heights. E5.1, E9.4, E12.7. |
| No software-rendering fallback on any platform, and Zed actively blocks lavapipe/llvmpipe/SwiftShader | Beyond "Linux needs Vulkan": **Windows RDP/VDI is effectively broken too**. `ZED_ALLOW_EMULATED_GPU=1` exists and is described as awful. If you ever want to run Snail over remote desktop, it won't. |
| Icons as raw SVG bytes via `svg().data(...)` | gpui-kit's `Icon` fixes its colour at render, so a button's hover cannot reach it. Both handoffs need hover-tinted icons. E1.6. |

Measured performance on the reference hardware, as the budget shape for Snail's lists: 40,000 rows
scrolled at 4000 px/s gave **draw p50/p99 of 1.76 / 2.86 ms** on Apple Silicon, and a steady 30 fps
on a Pi 5 at 1080p. A mail list is a strictly easier problem than a seven-column track table, so
these are ceilings Snail should beat.

### 1.3 One app, one design language — **decided: mail wins** (owner, 2026-09-20)

The two handoffs are two different products and nothing in either reconciles them:

| | Calendar (Almanac) | Mail |
|---|---|---|
| Ground | cool grey `#FAFAFA` / `#F4F4F4` / `#FFFFFF` | warm paper `#faf8f5` / `#f2efe9` / `#e8e4dc` |
| Accent | `#1F6FEB` | `#2c5fb8` |
| UI face | Bricolage Grotesque | Instrument Sans |
| Body face | — | Newsreader (serif), 15/1.7 and 16/1.75 |
| Mono | DM Mono | — |
| Window | frameless, custom 52px titlebar, drawn controls | native macOS traffic lights, 46/44px bars |
| Windows | one, 1440×900 | four, 1180×720 / 900×720 / 620×520 / 620×600 |

**The mail handoff is the design language. The calendar handoff is adjusted to it.**

Warm ground, mail accent, mail typography, calendar window chrome.

- **Window architecture comes from the calendar.** The mail mock's traffic lights are macOS-only
  and do not port; the calendar's frameless custom titlebar with drawn min/max/close is exactly
  what Ferrite proved works on both macOS and labwc. Cross-platform is a hard requirement, so this
  side of the fork decides itself.
- **Ground and accent come from mail.** `#faf8f5` warm paper with `#2c5fb8` is the more distinctive
  identity, and the serif reading pane is the mail handoff's entire thesis — "messages read like
  letters, not UI." The calendar's greys are generic; re-tinting them warm is mechanical work
  (a single hue shift on eleven neutrals), whereas re-deriving the mail design in cool grey would
  throw away the thing that makes it worth building.
- **Three families, not four.** Instrument Sans (UI) + Newsreader (message and event bodies) +
  DM Mono (times, dates, counts, section labels, shortcuts). **Bricolage Grotesque is dropped**;
  the calendar's display numerals (42px title, 76px day numeral, 46px agenda date) become
  Instrument Sans 600 with the same negative tracking. Self-hosted, bundled, no runtime fetch.
- **Dark mode is designed from scratch or not at all.** Neither handoff has a single dark token.
  See §5 open questions.

**The concrete restatement of the calendar handoff**, which E1.3 writes into the token table and
E12–E14 build against. Every calendar value on the left is replaced by the mail value on the right;
where the calendar has a role mail has no equivalent for, the mail ground is shifted to fill it.

| Calendar handoff value | Becomes | Note |
|---|---|---|
| `#FAFAFA` chrome / window base | `#f2efe9` | mail title bar / sidebar chrome |
| `#F4F4F4` sidebar | `#f2efe9` | mail and calendar sidebars unify |
| `#FFFFFF` surface | `#ffffff` | unchanged; mail's card colour is also white |
| `#FBFBFB` surface-2 (right rail, editor footer, agenda zebra) | `#f6f3ee` | mail's sunken row |
| `#FCFCFC` surface-3 (weekday strip, all-day band) | `#f6f3ee` | collapses to one sunken tone |
| `#E4E4E4` desk | `#e8e4dc` | mail desk |
| `#E0E0E0` border | `rgba(0,0,0,.09)` | mail card/sidebar border |
| `#E5E5E5` border-soft | `rgba(0,0,0,.07)` | mail chrome border |
| `#EBEBEB` grid (cell borders, column separators) | `rgba(0,0,0,.07)` | |
| `#F2F2F2` grid-hour | `rgba(0,0,0,.045)` | hour lines stay the faintest rule in the app |
| `#1A1A1A` ink | `#1b1917` | |
| `#232323` / `#262626` event title ink | `#1b1917` | one ink for event titles |
| `#424242` ink-2 (date numerals) | `#3f3a35` | mail secondary |
| `#575757` ink-3 (modal body) | `#241f1b` in Newsreader | event notes become serif, like message bodies |
| `#666666` ink-4 (place names) | `#6f6963` | mail muted |
| `#777777` ink-5 / `#888888` ink-6 | `#8a837b` | mail soft; two greys collapse to one |
| `#A0A0A0` ink-7 (section labels, hour gutter) | `#a09890` | mail faint |
| `#B3B3B3` ink-8 / `#BFBFBF` ink-9 | `#a09890` | mail faint absorbs both |
| `#CACACA` dashed | `rgba(0,0,0,.18)` | dashed add-affordances |
| `#D9D9D9` / `#E0E0E0` toggle track off | `#ddd8d0` | mail's toggle-off |
| `#1F6FEB` accent | `#2c5fb8` | |
| `#E4EBF8` accent-tint (mini-month selection, reminder chip) | `#e2eaf7` | mail's selected-row tint |
| — | `#d7e2f4` accent-tint-deep | mail's avatar/selected-sidebar fill; the mini-month **today** cell uses solid `#2c5fb8` as before |
| `#FBFCFE` today-wash / `#FDFDFE` today-wash-2 | `#faf8f5` | the warm canvas already reads as a wash against `#ffffff` cells |
| `#FCFBF8` out-of-month cell | `#f6f3ee` | resolves the README-vs-code conflict in mail's favour |
| `#144a9e` link hover | `#234e99` | mail's button-darken value |
| Bricolage Grotesque, all roles | **Instrument Sans**, same sizes/weights/tracking | 42/500/-0.03em, 76/400/-0.045em etc. all survive the swap |
| Calendar modal body copy | **Newsreader** 15/1.7 | event notes read like message bodies |
| DM Mono, all roles | **DM Mono**, unchanged | times, dates, counts, section labels, shortcuts |

**Calendar colours survive untouched.** `#C2410C` Classes, `#1F6FEB` Personal, `#0F766E` Work,
`#7C5CBF` Birthdays, `#A16207` Home and their 10% tints are per-calendar identity, not theme
chrome, and they sit well on warm paper. Note that Personal's `#1F6FEB` is now *distinct from* the
UI accent `#2c5fb8` rather than equal to it, which is an improvement: a calendar colour should not
be confusable with selection state. `#C2410C` keeps its double duty as danger/overdue, `#0F766E`
as synced-ok, `#A16207` as syncing, and the neutral offline dot becomes `#8a837b`.

**Radii and shadows unify on mail's scale** (5 / 6 / 7 / 8 / 9 / 11 / 12 / 20 / 50%). The
calendar's 14px modal radius becomes 12, its 15px reminder-chip radius becomes 20 (mail's
recipient-token radius, which is the same shape of object), and its 2/3/3.5 micro-radii stay.
Shadows come from mail: window `0 24px 60px rgba(40,32,20,.18), 0 2px 6px rgba(40,32,20,.1)`,
and the calendar's modal shadow warms to `0 24px 60px rgba(40,32,20,.34)`. Event-block shadows
(`0 1px 2px rgba(0,0,0,.05)`) and the segment-active shadow survive as-is.

**What is explicitly kept from the calendar handoff:** every geometry number. Pane widths (264
sidebar, 300 right rail, 236 settings nav), the 52px titlebar, header heights (88/78/56/106), hour
pitch per view (45/47/49px), the 6×7 month grid, the 132px agenda date block, the 576px editor
sheet, and all the spacing values. Only colour, type and radius are restated.

### 1.3b Dark theme — **decided: ship one** (owner, 2026-09-20)

Neither handoff contains a single dark token, so this is designed here rather than extracted. Two
rules keep it from becoming a second design system:

1. **It is a re-scale, not an inversion.** The mail palette's five surfaces already rank by
   elevation — `desk < chrome < sunken < canvas < card`. That ranking is *identical* in a dark
   theme (more elevated still means lighter), so the same token names keep the same meaning and
   only their values move into a dark range. Nothing in any view has to know which theme is active.
2. **It is neutral dark grey, not warm.** *(Owner, 2026-09-21, overriding the earlier "stays warm"
   rule.)* A brown-tinted dark ground read wrong. Dark Snail is a neutral near-black/dark-grey
   ground with neutral greys for text; the identity lives in the accent and the calendar colours,
   not the ground. There is no pure white anywhere, and the ground may go as dark as black.

**Surfaces** (same rank order as light, compressed into a dark range):

| Token | Light | Dark | Used by |
|---|---|---|---|
| desk | `#e8e4dc` | `#0a0a0a` | behind the window |
| chrome | `#f2efe9` | `#1a1a1a` | titlebar, sidebar, attachment chip |
| sunken | `#f6f3ee` | `#1e1e1e` | collapsed thread rows, format bar, agenda zebra, weekday strip |
| canvas | `#faf8f5` | `#121212` | message list, reading pane, calendar panes |
| card | `#ffffff` | `#262626` | settings cards, inline reply box, month cells, popovers |

**Text** — warm off-whites, never `#ffffff`:

| Token | Light | Dark |
|---|---|---|
| ink | `#1b1917` | `#f2f2f2` |
| body serif ink | `#241f1b` | `#e6e6e6` |
| secondary | `#3f3a35` | `#c8c8c8` |
| muted | `#6f6963` | `#a3a3a3` |
| soft | `#8a837b` | `#8a8a8a` |
| faint | `#a09890` | `#6e6e6e` |

Note `soft` barely moves: it is already a mid grey and reads correctly against both grounds.

**Accent** — `#2c5fb8` is too dark to sit on a dark ground, so it lifts, and the hover direction
**reverses** (light darkens to `#234e99`; dark lightens):

| Token | Light | Dark |
|---|---|---|
| accent | `#2c5fb8` | `#5b8ee0` |
| accent hover | `#234e99` | `#7aa5e8` |
| accent tint (selected row) | `#e2eaf7` | `#1e2c42` |
| accent tint deep (avatar, selected sidebar) | `#d7e2f4` | `#25344d` |
| accent text on tint (mailbox pill, token) | `#1d4489` | `#9cc0f2` |

**Borders** invert their base colour and keep their alpha ladder: `rgba(0,0,0,.055 / .07 / .09 /
.12)` becomes `rgba(255,255,255,.055 / .07 / .09 / .14)`. The top step widens slightly because
white-on-dark hairlines read fainter than black-on-light at the same alpha. The search field's
`rgba(0,0,0,.045)` fill becomes `rgba(255,255,255,.05)`.

**Odds and ends:** toggle-off track `#ddd8d0` → `#2e2e2e`; compose caret pipe `#c9c2b9` → `#4d4d4d`;
secondary account dot `#7a8f6d` → `#93a884`. Traffic lights are system colours and do not change.

**Calendar colours lift too.** These were picked for white and go muddy on dark, so each gets a
lighter sibling, and the 10% tints become ~20% alpha of the lifted colour (a 10% tint of anything
is invisible on `#232019`):

| Calendar | Light | Dark |
|---|---|---|
| Classes / danger / overdue | `#C2410C` | `#e2723f` |
| Personal | `#1F6FEB` | `#5b93f0` |
| Work / synced-ok | `#0F766E` | `#2fa89d` |
| Birthdays | `#7C5CBF` | `#a48ad8` |
| Home / syncing | `#A16207` | `#d09a35` |
| Offline / neutral | `#888888` | `#8a8279` |

**Elevation is carried by the surface scale, not by shadows.** Shadows barely read on dark, so the
window shadow stays (it separates the window from the desktop) while the modal and event-block
shadows drop to roughly half alpha and lean on `card` plus a `rgba(255,255,255,.09)` hairline
instead. The segment-active shadow is replaced outright by the surface step.

**One thing to watch in implementation:** Newsreader at 15/1.7 blooms on a dark ground. If the body
text reads heavier than it does in light, the fix is optical size and colour (`#e8e2da`, not
brighter), **never a lighter font weight** — Newsreader's lighter cuts lose the letterform that the
serif reading pane exists for.

Theme selection follows the OS by default (`System` / `Light` / `Dark`), which E1.12 already wires
through `cx.observe_window_appearance`. **Light remains the reference theme**: it is what both
handoffs specify and what the pixel-diff harness (E17.7) compares against. Dark gets its own
contrast pass rather than its own pixel baseline.

### 1.4 HTML email: a custom renderer, no webview

Chosen by the owner, and it is the right call for this product — a webview is the single largest
thing standing between "lightweight" and "Thunderbird". The cost is real and is scoped as its own
epic (E6). The pipeline:

```
raw MIME part ──► html5ever parse ──► ammonia sanitize (allowlist, strip <script>, <iframe>,
                                      on* handlers, remote URLs unless unblocked)
              ──► inline the <style> and style="" we support (a restricted CSS subset)
              ──► our own box/inline layout over the subset
              ──► GPUI elements, text shaped by window.text_system()
```

**The subset is the whole design.** Email HTML is 1997 markup: nested tables for layout, inline
styles, `<font>`, spacer GIFs, and the occasional modern `<div>` newsletter. E6 targets that, not
the modern web. Anything outside the subset degrades to readable text rather than breaking, and
there is always an escape hatch: "Open in browser" hands the sanitized document to the system
browser.

**Remote content is blocked by construction**, which the mail handoff already asks for ("Off keeps
senders from knowing you opened a message"). A custom renderer makes that a property of the
architecture instead of a setting that a webview might leak around.

Structural insurance: `ferrite-viz` in the reference project is a working template for a
**separate helper process** hosting a webview, talking to the parent over a loopback HTTP+WS bridge
with a per-launch token. If E6 proves unaffordable, that is the fallback, and it keeps the webview
out of Snail's own process. It is not v1 scope.

### 1.5 Store and secrets

- **SQLite via `rusqlite` with `bundled`**, one connection behind a `Mutex`, touched only from the
  background executor. Schema as one `CREATE TABLE IF NOT EXISTS` batch plus a version row in a
  `meta` table, exactly as Ferrite does it. No sqlx, no diesel, no async DB.
- **FTS5** for message search. Chosen over tantivy: it is already linked, it stays transactionally
  consistent with the messages table (no second index to repair after a crash), and a personal
  mailbox is the size where FTS5's simplicity wins. Revisit only if E9 misses its budget.
- **Settings as versioned JSON files**, one per section, written atomically (temp + rename), so
  saving the compose draft never rewrites the theme and a crash mid-write leaves the old file
  intact. A bad value logs a warning and falls back to the default; it never fails startup.
- **Secrets in the OS keychain** via `keyring` (`apple-native`, `windows-native`,
  `sync-secret-service`). OAuth refresh tokens, iCloud app-specific passwords, and the PGP key
  passphrase if the user chooses to store it. Nothing secret ever lands in SQLite or JSON.
- **`SNAIL_CONFIG_DIR` / `SNAIL_CACHE_DIR` env overrides on day one**, so benchmarks, fixtures and
  screenshot runs never touch the real mailbox. Bundle id `dev.snail.app` fixes the directory name
  on all three platforms.
- Cache (message bodies, attachments, avatars) lives in the **cache dir, not Application Support**
  — otherwise a multi-gigabyte mail store gets backed up to iCloud.

### 1.6 PGP library — **decided: `pgp` (rpgp), plus a semantics layer we own**

`pgp` (rpgp) 0.20.0, MIT/Apache-2.0, over `sequoia-openpgp` 2.4.1, LGPL-2.0-or-later.

**The build story decides it, and it is not a matter of taste.** Sequoia still defaults to
`crypto-nettle` — a C library — and every escape route costs something concrete on a three-platform
single binary:

| Sequoia backend | Cost |
|---|---|
| `crypto-nettle` (default) | C dependency; on Windows means MSYS2 and a MinGW toolchain fighting the MSVC target; cross-compiling means cross-compiling Nettle and GMP |
| `crypto-cng` (what its README recommends for Windows) | **a different crypto backend on Windows than on macOS/Linux** — and the changelog shows algorithm support landing per-backend at different times, so a message could decrypt on one machine and not another |
| `crypto-openssl` | vendoring OpenSSL on all three |
| `crypto-rust` | gated behind `allow-experimental-crypto` and `allow-variable-time-crypto`, and its own README says the RustCrypto crates "are not recommended for general use" |

rpgp is pure Rust — its 68 dependencies contain no `*-sys` crate — and its CI builds both
`x86_64-pc-windows-msvc` and `-gnu` plus cross-compilation with no system-package setup at all. For
a solo developer that difference compounds on every release.

**The licence is also worse than sequoia's own field suggests.** `sequoia-openpgp` is LGPL-2.0+, but
its *default backend* is not: `nettle-sys` is `LGPL-3.0-only OR GPL-2.0-only OR GPL-3.0-only` —
**there is no LGPL-2.x branch**, so the most permissive option is LGPL-3.0, which is stronger than
the crate it serves. A default-configuration build is governed by the stricter Nettle terms.
Irrelevant if this stays personal and open; it is the kind of thing that is expensive to discover
late.

**Two more findings that cut against sequoia**, both from its own materials:
- **Sequoia has never been security-audited** — its status page says so directly, citing lack of
  funding. **rpgp has been**, by Radically Open Security under an NLnet grant, and that audit is
  what produced RUSTSEC-2024-0447. The usual intuition that the more institutional project is the
  more scrutinised one is backwards here.
- **Sequoia's funding is a maintenance risk.** The only documented funding — €900k from Germany's
  Sovereign Tech Fund — is marked *completed* as of 2024, no current funder could be verified, and
  the bus factor looks like one or two people.

**In fairness, rpgp's maintenance signal is not strong either, and this is the weakest part of the
choice.** Two stable releases in 2026 against four in 2025; a 51-day gap with nothing merged
followed by two more weeks of quiet; **one** crates.io owner and **one** security contact; a
SECURITY.md that describes the project as "maintained by a team of volunteers on a reasonable-effort
basis"; no funding channel at all, and the NLnet grant that paid for RFC 9580 support has run its
scope. Sixty open issues reach back to 2019. Delta Chat and Proton both pin the current release and
contribute upstream, which is real, but neither is documented as *funding* it. **Both libraries are
thinly resourced; neither choice buys institutional durability.**

**Three operational facts that come with rpgp and belong in the plan, not in a postmortem:**
- **`cargo audit` will not protect you here.** rpgp has had six security advisories; **RustSec
  carries one of them**, and the one currently *unpatched* advisory — unbounded decompression
  amplification in Compressed Data Packets, affecting `<= 0.20.0` — is in neither OSV nor the GitHub
  Advisory Database. It exists only on the repo's own advisories page. E10.9 makes watching that
  page an explicit chore.
- **There are no security backports.** SECURITY.md: "Security updates are applied only to the most
  recent release." Since every `0.x` bump is a breaking bump under Cargo semver, **a CVE fix is an
  API port**. Budget roughly one per quarter; 0.19.0 alone carried eleven tagged breaking changes.
- **A known, unresolved timing side-channel is inherited**: the `rsa` crate's Marvin attack
  (RUSTSEC-2023-0071), which rpgp self-discloses and whose advisory its own `deny.toml` ignores.
  Note also that the 2024 Radically Open Security audit — the one genuine point of assurance rpgp
  has over sequoia — was a 9-day fuzz-and-static review that **explicitly excluded side channels**,
  and targeted a version six breaking minors old.

Adoption seals it: **Delta Chat ships rpgp**, pinned to the *current* release, on exactly these
platforms, with its developers committing upstream.

**The honest cost, and it is the largest thing this decision hands us.** rpgp's own README states it
implements the wire format, composite objects and signature/encryption processing, but
**"explicitly does not deal with"** OpenPGP *semantics*: expiration, revocation, key flags,
algorithm preferences. For a mail client that list **is** the security-relevant behaviour — rpgp
will hand back a signature from a revoked key, an expired subkey, or a subkey with no signing flag
as valid, without complaint, and will happily process MD5 and SHA-1 artifacts. Sequoia's
`StandardPolicy` gives exactly that layer, expert-curated with dated algorithm cutoffs (MD5 rejected
from 2004, SHA-1 from 2023, sub-2048-bit keys from 2014) and threaded through the API as a
*required argument* — skipping it is a compile error, not a silent default.

**But sequoia is less automatic than that makes it sound, and the gap between the two is narrower
than it first appears.** Its key-selection filters are **none of them on by default**: a bare
`.keys()` returns everything including dead and revoked keys, `revoked()` does not check the
certificate's own status, and the docs warn about this in three separate places — which tells you
how often it is got wrong. `ValidCert` guarantees only that the *binding signature* is live, and
says so explicitly: "it says nothing about whether the certificate or any component is live. If you
care about those things, then you need to check them separately." A `VerificationHelper::check` that
returns `Ok(())` compiles, runs, and verifies nothing, and the floor for a correct decrypt-and-verify
is roughly 100–150 lines of helper plumbing. So sequoia hands us a curated *algorithm* policy and a
hard-to-bypass policy argument; the certificate-semantics checking is substantially ours either way.

So E10.3b below is a real deliverable, not a footnote: **we own the validity layer.** `rpgpie`
0.11.1 (MIT/Apache-2.0, by rpgp's own lead maintainer, and pointed to from rpgp's README) implements
precisely this gap — but on a closer look it is a weaker crutch than it first appears: its policy is
**hard-coded, not caller-configurable** (the cutoff constants are private and the decision functions
are `pub(crate)`), and its own docs concede the timestamp-cutoff model can be gamed, since "an
attacker may trick users with weak, new (or newly modified) artifacts that show 'old' signature
creation timestamps." Treat it as the **reference implementation to read and test against**, and
lean toward our own explicit layer so the policy is ours to inspect and tune.

One calibration point worth keeping honest: rpgp ranks **first** on the OpenPGP interoperability
test suite (96% of individual vectors, ahead of GopenPGP, Sequoia, RNP and GnuPG) — but **that entry
is `rpgpie-sop`, not raw rpgp.** It passes all 32 revoked-key vectors and the certificate-expiration
vectors precisely *because* rpgpie supplies the layer rpgp omits. Raw rpgp would fail every one of
them. The ranking is an argument for the stack, not for the base crate alone. **If that layer ever looks like more than it is
worth, sequoia-with-Nettle is the correct fallback and this decision should be revisited rather than
patched around.**

Two facts that apply whichever library wins: **there is no PGP/MIME crate for either** — confirmed
against all 68 projects in sequoia's own GitLab group, and `openpgp-mime` / `pgp-mime` / `mail-pgp`
simply do not exist on crates.io — so RFC 3156 canonicalization is ours regardless; and
**both libraries still ship panic-on-malformed-input bugs** — every RUSTSEC advisory for both is a
DoS of this class, including one in rpgp fixed as recently as 2026-07. Untrusted mail gets parsed
behind a catch-unwind boundary no matter what.

---

## 2. Architecture

```
┌──────────────────────────── snail (GPUI, one process) ────────────────────────────┐
│ Views (Entity<…>): TitleBar  Sidebar  MessageList  ReadingPane  ThreadView         │
│   Compose  MonthGrid  WeekGrid  DayPane  Agenda  EventEditor  Settings  Search     │
│ Models: MailModel  ThreadModel  CalendarModel  AccountsModel  SyncModel            │
│         ThemeState  CommandModel                                                   │
│        ▲ entity.update(...) from one cx.spawn bridge per event stream              │
│        │                                                                            │
│ snail-services (UI-agnostic): SyncScheduler  AccountStore(keyring)  SettingsStore  │
│   NotificationService  Hub<SyncEvent>  attachment/temp-file handling                │
│        │                                                                            │
│ snail-core: store (SQLite+FTS5), MIME, threading, HTML→DOM, recurrence, PGP,        │
│   providers: GmailProvider  GoogleCalendarProvider  ImapProvider  CaldavProvider    │
└────────────────────────────────────────────────────────────────────────────────────┘
```

### Workspace layout

| Crate | Contents | Depends on |
|---|---|---|
| `crates/snail-core` | Store, MIME parse/build, threading, recurrence, PGP, the four provider clients, HTML sanitize + DOM. No UI, no GPUI. | — |
| `crates/snail-ui` | **Pure UI model with no UI framework.** Theme/palette maths, text roles, the command + shortcut table, list selection and keyboard-nav rules, calendar grid geometry (month cells, time-grid pitch, event block placement, overlap columns), date formatting, HTML **layout** (box tree → positioned boxes, given a font-metrics trait). Only deps `serde`/`serde_json`. | — |
| `crates/snail-services` | Sync scheduling, account/secret storage, settings, notifications, the `Hub<T>` event bus. | core |
| `crates/snail` (bin) | GPUI views, models, theme application, actions/keymap. | all three |

`snail-ui` is the load-bearing idea, copied from Ferrite where it holds 201 of the project's 710
tests. **Everything that can be decided without pixels is decided there and tested in seconds
without building GPUI.** For this app that is a lot: which events go in which column of an
overlapping day, where a 90-minute block lands at 45px-per-hour pitch, which messages form a
thread, what `↑` does with a multi-selection, how a 10-minute-before reminder resolves across a DST
boundary, and how a nested `<table>` lays out. GPUI compiles slowly; none of that should wait on it.

### Principles

- **Rust owns the data; views read it directly.** No projection layer, no precomputed display
  strings unless profiling asks for them. Row text is formatted on paint from cached
  `SharedString`s, invalidated per message id.
- **Nothing blocks the main thread.** Every network call, SQLite read, MIME parse, HTML layout,
  recurrence expansion and PGP operation runs on `cx.background_executor()`, landing through
  `cx.spawn` + `entity.update` with a **generation guard** so a stale result is dropped rather than
  applied. PGP in particular is deliberately slow by design and must never touch the UI thread.
- **Sync threads reach the UI through one bridge.** Providers publish to a `Hub<SyncEvent>`; the
  GPUI side subscribes with a closure pushing into a `futures::mpsc::unbounded`, drained by a
  `cx.spawn` loop that exits when `entity.update` returns `Err`. High-rate progress uses the
  latest-wins `Latest<T>` variant so the UI never queues behind a sync.
- **The provider difference stops at the crate boundary.** `MailProvider` and `CalendarProvider`
  traits; Gmail-over-REST and iCloud-over-IMAP produce the same `Message` rows, Google Calendar
  and iCloud CalDAV the same `Event` rows. **No view ever branches on the account kind** except to
  draw the account's name and colour dot.
- **Offline is the normal case, not an error state.** Every read is served from SQLite. The network
  only ever fills it or drains a queue of pending local changes.
- **Optimistic local writes with rollback.** Archive, trash, mark-read, RSVP and event edits apply
  to the store and the UI immediately, enqueue a remote operation, and roll back visibly if the
  server rejects it. The mail handoff's undo affordance is the user-facing half of the same
  mechanism.

---

## 3. Epics and stories

Story format: `[ ] N.n — story`. Epics are ordered by dependency, not by priority. **E0–E7 make a
mail client you can live in; E8–E11 make it good; E12–E15 add the calendar; E16–E20 make it
shippable to one person on three machines.**

Every story that touches a pane, a row or a glyph is checked against the handoff values in §1.3.
Every story that touches the network is checked with the network off.

### E0 — Foundations and go/no-go spikes
*Nothing else starts until E0.2, E0.3 and E0.4 pass. Each is a throwaway binary in `spikes/`, in
its own workspace so the app's lockfile is untouched, and each writes its result into
`spikes/RESULTS.md` the way Ferrite's G0 did.*

- [x] 0.1 — Workspace scaffold: `snail-core`, `snail-ui`, `snail-services`, `snail` (bin), resolver
      2, `[profile.dev.package."*"] opt-level = 2`. Opens an empty window on macOS with the pinned
      gpui-kit family, `.with_assets(gpui_kit::assets::Assets)`, and
      `window_decorations: Some(WindowDecorations::Client)`. *(Done; gpui-kit 0.6.4 / gpui-pre
      0.3.5, see `spikes/RESULTS.md` E0.1.)*
- [ ] 0.2 — **Spike: gpui-kit `Input`/`Textarea` under real load.** The biggest single dependency
      risk in the plan (§1.1). Build a throwaway compose window: To/Cc/Subject inputs, a multi-line
      body `Textarea`, tab order between them, IME (test with a CJK input method on all three
      platforms), select-all/copy/paste/undo, placeholder, blur/focus events, and a 40-line body
      with soft wrap. Record which of these work, which need patching, and what the `InputEvent`
      surface actually is at 0.6.1. **Go/no-go:** if `Textarea` cannot carry a compose body, the
      fallback is writing one text-input element against GPUI's `text_system` directly, which is
      three weeks of work and must be known now, not in E7.
- [x] 0.3 — **Spike: the HTML renderer's hardest real input.** Take 20 messages from the owner's
      actual inbox — deliberately weighted to the worst: a nested-table marketing newsletter, a
      GitHub notification, a Google Calendar invite, a quoted-reply chain five levels deep, an
      `apple-mail`-generated reply, a plain-text message with format=flowed, and one with inline
      `cid:` images. Sanitize, lay out, and render them. **Go/no-go:** all 20 must be *readable*
      (not pixel-perfect) and lay out in under 16 ms for a screenful. Output the failure taxonomy
      — it is E6's backlog.
- [x] 0.4 — **Spike: both accounts authenticate and pull one page.** A CLI that runs the Google
      loopback OAuth+PKCE flow, and separately connects to iCloud IMAP with an app-specific
      password from the keychain, and prints 50 message headers from each. Plus one Google Calendar
      page and one iCloud CalDAV report. **Go/no-go:** proves the auth story end to end before any
      UI exists, and pins down the scope list Google actually grants. *(Details in E3/E4/E12; see
      the protocol notes in §6.)*
- [x] 0.5 — App identity and paths: bundle id `dev.snail.app`, config/cache/log dirs via `dirs`,
      `SNAIL_CONFIG_DIR` / `SNAIL_CACHE_DIR` overrides, rotating log file. Cache is separate from
      config so the mail store is never iCloud-backed-up.
- [ ] 0.6 — Google Cloud project configured as an **unverified app published to production**, not
      testing mode. This is the personal-tool decision cashed in — no verification, no CASA, no
      demo video — but the choice between Google's two unverified states matters a lot and the
      obvious one is wrong:

      | | Testing mode | Production, unverified |
      |---|---|---|
      | Consent | clean, test users only (max 100) | "Google hasn't verified this app" interstitial, once, via Advanced → Go to app |
      | **Refresh token life** | **expires 7 days after consent** | normal (no fixed expiry) |
      | User cap | 100 test users | 100 users for restricted scopes |
      | Verification | exempt | exempt under the personal-use exception (<100 users) |

      **Testing mode's 7-day refresh-token expiry makes it unusable for a daily driver** — the app
      would demand a browser round-trip every week forever. Production-unverified costs one ugly
      warning screen at setup and then behaves normally. Verify this empirically in E0.4 by
      checking a refresh token still works on day 8; it is the kind of thing Google changes.
      **Fallback if production-unverified is refused for restricted scopes:** iCloud-style IMAP
      against Gmail using an app password — which Google also gates behind 2FA and may withdraw —
      or accept the weekly re-auth. Know which before E3 is built.
- [x] 0.7 — Startup timeline instrumentation: `startup::mark("gpui_init" | "fonts" | "services" |
      "models" | "window_opened" | "shell_built" | "first_frame")`. Cheap enough to leave on
      permanently; it is how a 60 ms blocking read gets found.
- [x] 0.8 — Dev overlay: frame p50/p99 from gpui-kit's `profiler` feature, visible row count, last
      sync duration, store size, RSS. Every later budget is read off this. *(Done; `SNAIL_OVERLAY=1`
      or F2. Row/store/sync are placeholders until E2/E4.)*
- [x] 0.9 — Fixture generator: a synthetic store with 200k messages across 8 mailboxes and 20k
      events, plus a `--fixture` mode that points the app at it. All list and search benchmarks run
      against this, never against the real mailbox. *(Done; `snail --generate-fixture` writes it in
      ~3 s, `--bench-store` reports. Deterministic, so `--bench-compare` is meaningful.)*
- [x] 0.10 — CI matrix from day one: GitHub Actions on `macos-latest`, `ubuntu-latest`,
      `windows-latest`, building **the GPUI binary itself** on all three (the reference project's
      CI only builds it on macOS, and that is explicitly the gap to not repeat) plus
      `cargo test --workspace`. Ubuntu runner needs the Vulkan and portal packages listed in
      BUILDING.md. **Green on all three, 2026-09-21 (run 35553496010):** `cargo build` + `cargo test`
      on macOS, Ubuntu and Windows, and all three spikes build on all three. The Ubuntu runner
      installs the Vulkan loader and the X11/Wayland/font build deps; BUILDING.md's package list is
      E18.4.
- [ ] 0.11 — Test hardware available: a Linux machine **with a working Vulkan driver** (§1.2 — this
      is not optional) and a Windows machine. Both with a checkout that builds. *(The Pi 5 at
      `neo.local` builds `cargo build --workspace` in 30m50s, verified 2026-09-21, with V3D hardware
      Vulkan; it has no display attached, so the GUI/IME pass is deferred by the owner. Windows
      still pending.)*

### E1 — App shell, window chrome and the design system

- [ ] 1.1 — `gpui_kit::application().with_assets(gpui_kit::assets::Assets)`, `gpui_kit::init`,
      fonts registered from `include_bytes!` before the window opens, `Root::new(...)` as the root
      element so popovers, dialogs and context menus have somewhere to render.
- [ ] 1.2 — Frameless window: 1240×820 default, 900×600 minimum, 12px radius, mail's window shadow.
      52px titlebar as the OS drag region. macOS gets transparent titlebar + real traffic lights at
      the handoff's position; Windows and Linux get drawn min/max/close plus Snap Layouts regions
      on Windows. `WindowDecorations::Client` set explicitly.
- [ ] 1.3 — `theme.rs`: every colour, size, radius and shadow from §1.3's restated table as named
      tokens, in **two palettes** (§1.3b) behind one token set. **Views never write a raw hex or a
      raw text size** — enforced by keeping `Rgba` out of view modules and routing all text through
      1.4. A `#[test]` asserts both palettes define every token, so a dark value can never be
      silently missing.
- [ ] 1.3b — Contrast guard: a test computing WCAG contrast for every (text token, surface token)
      pair actually used, failing below 4.5:1 for body text and 3:1 for large text and UI
      boundaries — **in both themes**. This is how a dark theme stops rotting the first time
      somebody nudges a grey.
- [ ] 1.4 — `TextRole` enum in `snail-ui` (family / size / weight / line-height / tracking /
      uppercase / optional colour) covering every role in both handoffs — the ~30 mail roles and
      ~40 calendar roles, deduplicated. `style::text(content, role, palette) -> Div` is the only
      way a view produces text. Unit-tested: every role resolves, no role is unused.
- [ ] 1.5 — Letter-spacing element (§1.2): shape once via `window.text_system().shape_line`, then
      position each glyph at `line.x_for_index(byte) + spacing * position`. Needed for DM Mono
      section labels at 0.09–0.12em and display type at -0.015 to -0.045em. **Hard-scoped to short
      single-line non-editable labels** — one element per glyph is correct for `MAILBOXES` and
      catastrophic for a message list.
- [ ] 1.6 — Icon set as raw SVG byte consts via `svg().data(...)`, not gpui-kit `Icon` (§1.2), so
      hover can re-tint them. The handoffs need: envelope, archive, trash, reply, forward, back
      chevron, download, search, document, link, list, bold, italic, disclosure chevron, inbox,
      paper-plane, file, plus calendar, clock, repeat, bell, plus, x, chevrons, three-dot overflow,
      lock/shield (PGP). 16×16 grid, 1.4–1.5 stroke.
- [ ] 1.7 — Map the palette onto gpui-kit's `Theme` (background, border, list_hover, title_bar,
      scrollbar_thumb, drop_target, popover, input, …) **and set `ThemeMode::Dark` handling
      correctly from startup** — §1.2. gpui-kit sizes in rems off `theme.font_size`; set it so
      their components land on our scale.
- [ ] 1.8 — Shell layout: titlebar → `[ sidebar 206 | content ]`, where content is the mail
      three-pane or a calendar view. Sidebar unifies the mail handoff's (mailboxes + accounts) and
      the calendar handoff's (mini-month + calendars + tasks + sync footer) into one 206–264px
      column whose contents switch with the active surface. **Width: 232px**, splitting the
      difference; the mini-month's 7×27px grid is the binding constraint.
- [ ] 1.9 — **The states matrix.** The single biggest undesigned surface in both bundles: neither
      mockup draws hover, focus, empty, loading, error or disabled. Produce one table covering every
      interactive component × every state, using the mail README's specified values (`rgba(0,0,0,.03)`
      row hover; filled button darkens to `#234e99`; outlined border to `rgba(0,0,0,.2)`; focus ring
      2px `#2c5fb8` at 40% alpha, offset 2) extended to the calendar's components — **and with a
      dark column**, where the hover tint becomes `rgba(255,255,255,.04)` and every "darkens to"
      becomes "lightens to" (§1.3b). This is a design
      artifact, reviewed before E5 builds against it.
- [ ] 1.10 — Empty, loading and error states drawn for: no accounts yet, mailbox syncing for the
      first time, empty mailbox, no search results, message body failed to parse, offline, sync
      error, and calendar range with no events. Also undesigned in both handoffs.
- [ ] 1.11 — Motion: 120–180 ms ease-out for view changes and sheet entry (the calendar README's
      prescription), and **nothing else anywhere**. No row transitions, no hover fades — hover is
      the one thing that must be instant.
- [ ] 1.12 — Theme selection `System` / `Light` / `Dark`, persisted in settings, following the OS
      via `cx.observe_window_appearance` when set to System. Switching re-maps the gpui-kit `Theme`
      (1.7), pins native chrome with `cx.set_window_appearance`, and calls `cx.refresh_windows()`.
      Must be **live** — no relaunch — because the OS can flip it under us at sunset.

### E2 — The local store

- [x] 2.1 — SQLite schema v1 as one `CREATE TABLE IF NOT EXISTS` batch plus `meta(key, value)`
      versioning and a migration test that walks v1→v2 on a fixture. Tables: `account`, `mailbox`,
      `message`, `message_part`, `thread`, `attachment`, `label`, `message_label`, `pending_op`,
      `calendar`, `event`, `event_exception`, `reminder`, `task`, `sync_state`.
- [x] 2.2 — `message` carries both provider identities without either leaking upward: Gmail's
      `id`/`thread_id`/`history_id` and IMAP's `uid`/`uidvalidity`/`modseq` live in nullable
      columns behind one `ProviderRef`. RFC822 `Message-ID`, `In-Reply-To` and `References` are
      stored for every account kind because threading needs them regardless.
- [x] 2.3 — Body storage: headers and a plain-text preview in SQLite; **full bodies and attachments
      as content-addressed files in the cache dir**, referenced by hash. Keeps the DB small enough
      to stay fast and makes "clear cache" a directory delete. A missing cache file re-fetches
      rather than erroring.
- [x] 2.4 — Connection handling: one `rusqlite` connection behind a `Mutex`, WAL mode, accessed only
      from the background executor via `store.with_db(|db| ...)`. A `#[test]` that fails if any
      store call is reachable from a view module.
- [x] 2.5 — `pending_op` queue: every local mutation (mark read, archive, trash, label, send, event
      create/update/delete, RSVP) is written as a row with account, target, operation, attempt
      count and an idempotency key **in the same transaction as the optimistic local change**.
      Survives restart; drains when online; surfaces as "N changes pending" in the sync footer.
- [x] 2.6 — `sync_state` per (account, kind): Gmail `historyId`, Google Calendar `syncToken`, IMAP
      `uidvalidity`+`highestmodseq`, CalDAV `sync-token`/`ctag`. Plus a `full_resync_needed` flag,
      because every one of these can be invalidated by the server and the recovery path must be a
      first-class state rather than an error.
- [x] 2.7 — Settings store: versioned JSON, one file per section (`theme`, `layout`, `accounts`,
      `mail`, `calendar`, `pgp`), atomic temp+rename writes, warn-and-default on a bad value.
- [x] 2.8 — Secret store over `keyring`: OAuth refresh tokens, iCloud app-specific passwords, PGP
      passphrase. One trait so tests use an in-memory backend. Handles the "no keyring daemon"
      Linux case with a clear error rather than a panic — see §6.
- [x] 2.9 — Store benchmarks against the E0.9 fixture: cold open, mailbox page query, thread
      assembly, unread counts. Budgets in §4. *(Measured 2026-09-21 on the 200k fixture: cold open
      2.0 ms, mailbox page p50/p99 0.11 / 3.80 ms, thread assemble 0.58 ms, unread counts 0.23 ms —
      the last only after finding that `count(*)` took **138 ms**; the sidebar reads the `unread`
      counter instead. Fixture: 96 MB for 200k messages.)*

### E3 — Accounts and authentication
*Gated on the E0.4 spike. The two providers share nothing here, which is exactly why the trait
boundary sits above this epic.*

- [x] 3.1 — Google OAuth 2.0 for installed apps, client type **Desktop app**: **loopback redirect**
      to `http://127.0.0.1:<port>` on a random free port, with **PKCE**, opened in the **system
      browser**. Google's policy forbids embedded user-agents (this has broken shipped clients), and
      the OOB copy-paste flow has been fully blocked since 2023-01-31. A transient local HTTP server
      catches the code, shows a "you can close this tab" page, and shuts down. Three details that
      are easy to get wrong:
      - Use `127.0.0.1` (and `[::1]`), **not `localhost`** — Google notes some client firewalls
        break loopback by hostname. Loopback matching ignores the port, so an ephemeral port is fine.
      - **Send `code_challenge_method=S256` explicitly.** Google documents PKCE as *recommended*
        rather than required, and `code_challenge_method` **silently defaults to `plain`** when a
        `code_challenge` is sent without it. A plain challenge is barely better than none.
      - The client secret ships in the binary and cannot be confidential — Google states installed
        apps "cannot keep secrets." Treat it as a public identifier. Note the tension: Google's
        policy page separately says credentials must *never* be committed to a public repo, with no
        installed-app exception, so if this repo ever goes public the secret should be injected at
        build time rather than committed.
- [x] 3.1b — **Guard against OAuth client auto-deletion.** Since June 2025 Google deletes OAuth
      clients unused for 6 months (restorable for 30 days), warning by email to the project owner —
      i.e. the address nobody reads on a side project. A deleted client fails every existing refresh
      token with `deleted_client`. Since Snail is used daily this should never trigger, but the
      error deserves its own message so a months-idle machine explains itself instead of looking
      like a generic auth failure. Also note secrets are now hashed and shown **once** at creation —
      record it in the password manager immediately, because the console will only ever show the
      last four characters again.
- [x] 3.2 — Scope decision, made once and written down (§6): **Gmail REST API with `gmail.modify`
      and `gmail.labels`, plus `calendar` and `calendar.events`.** Not IMAP-against-Gmail. Both
      routes are equally *restricted* for verification purposes, so the choice is on merits:
      `history.list` delta sync is far cheaper and more reliable than IMAP CONDSTORE, there is no
      15-connection ceiling, and it dodges the Workspace OAuth-client allowlist. The cost is that
      Gmail and iCloud need two different sync implementations, which E4 accepts by design.
- [ ] 3.3 — Token lifecycle: refresh tokens in the OS keychain, refreshed on a background task
      ahead of expiry, with a distinct terminal state for revoked/expired that prompts once.
      Verify the day-8 behaviour from E0.6 empirically before relying on it. *(Done:
      `GoogleOAuth::refresh` and keychain storage of the refresh token via `AccountStore`.
      Remaining: the background refresh-ahead task and the once-only revoked prompt, which land
      with E16.9; the day-8 check itself is E0.6.)*
- [ ] 3.4 — iCloud authentication: **app-specific password**, entered once and stored in the
      keychain. There is no OAuth available to third parties — iCloud IMAP advertises only
      `AUTH=ATOKEN` and `AUTH=PLAIN`, and `ATOKEN` is Apple's undocumented partner-only delegated
      flow, not something an independent developer can obtain. Sign in with Apple is unrelated and
      grants no mail access. Setup UI must say: generate at **account.apple.com** (not the old
      `appleid.apple.com`) → Sign-In and Security → App-Specific Passwords; **two-factor auth is
      mandatory** on the Apple Account; there is a **25 active password limit**; and — the one that
      produces mystifying support cases — **changing the Apple Account password silently revokes
      every app-specific password**. That last case must produce "your app password was revoked,
      generate a new one", never a generic auth failure. *(Done: storage in the keychain, plus
      distinct wrong-password and revoked messages. Remaining: the setup copy itself, in E14.3's
      add-account flow.)*
- [x] 3.4b — Username asymmetry, which Apple documents and every client gets wrong once: the
      **IMAP** username is the *name part only* (`johnappleseed`), the **SMTP** username is the
      *full address* (`johnappleseed@icloud.com`). Try the full address first on both and fall back
      to the local part on IMAP auth failure, and keep the field user-editable — `@me.com`/`@mac.com`
      legacy accounts are under-documented here.
- [x] 3.4c — **Identities are separate from accounts.** iCloud+ custom domains and `@me.com`/
      `@mac.com` aliases are additional send-as addresses on one credential, not separate accounts.
      Model a `identity` table (display name, address, signature, default) hanging off `account`
      from the first commit; retrofitting this is painful.
- [ ] 3.5 — iCloud endpoint discovery rather than hardcoding: IMAP and SMTP hosts, and CalDAV via
      `/.well-known/caldav` → current-user-principal → calendar-home-set. Hardcoded values are the
      fallback, not the primary path. *(The design handoff's own account card already shows
      `caldav.icloud.com`.)* *(Not started: the E0.4 spike proved the CalDAV discovery chain
      end to end — PROPFIND `.well-known`, principal, sharded home; formalizing IMAP/SMTP/CalDAV
      discovery with a hardcoded fallback is outstanding.)*
- [x] 3.6 — Distinct, actionable errors for every auth failure mode, because "login failed" is
      useless here: wrong app password, 2FA not enabled on the Apple ID, Google token revoked,
      Google consent withdrawn, Workspace admin has disabled the app or allowlisted other OAuth
      clients, network unreachable, and clock skew (which breaks OAuth and is invisible otherwise).
- [x] 3.7 — Multi-account from the start — not retrofitted. Every store row, every sync cursor and
      every view is account-scoped from the first commit, and the fixture has two accounts.
- [ ] 3.8 — Keyring failure handling: on Linux with no Secret Service available, say so clearly and
      offer an explicit, clearly-labelled encrypted-file fallback rather than crashing or silently
      storing plaintext. *(Partial: a missing keychain maps to a clear, actionable error. Remaining:
      the encrypted-file fallback.)*

### E4 — Mail sync
*Two implementations behind one `MailProvider` trait. All of it in `snail-core`, driven by
`snail-services`, invisible to every view.*

*Implemented: the `MailProvider` trait (4.1), MIME parsing (4.22), Gmail's pure logic (4.3–4.6, 4.8,
4.9), iCloud folder mapping (4.18), and the **live Gmail REST client and backfill**, proven end to
end on 2026-09-21: a 669,883-message account, 150 messages of the last 30 days **filed into
Inbox/Sent by their Gmail labels** with threads and mailbox counters, raw MIME cached, head taken
before enumerating (4.2), and retry-with-backoff surviving a **real per-user quota 403** (4.8/E16.4).
Remaining: applying history records incrementally to the store (4.7), the IMAP client
(4.10–4.17, 4.19–4.21), SMTP send, and the kill-safety tests (4.23/4.24).*

**Gmail (REST):**
- [x] 4.1 — `MailProvider` trait: `list_changes(cursor) -> Changes`, `fetch_messages(ids) -> Vec<Raw>`,
      `apply(ops)`, `send(raw)`. Both providers implement it; the store and UI know nothing else.
- [x] 4.2 — Initial backfill: take the mailbox head from `getProfile` **before** enumerating, then
      enumerate, then start incremental from that head. Doing it in the other order silently loses
      every change that lands during the backfill — the single most common bug in Gmail clients.
      **Scope the backfill to the last 30 days** (owner, 2026-09-21; §8 open question 3) — a
      669,873-message mailbox makes a full pull a ~37-hour floor (§6.5) — with older mail fetched on
      demand. Head-first ordering still applies within the window.
- [x] 4.3 — Fetch bodies as **`threads.get?format=raw`** and store the original RFC822 bytes.
      Rationale: 40 units per thread beats 20 × N messages at 3+ messages per thread; byte fidelity
      is required for PGP signature verification (E10) and for round-tripping; and it makes
      `messages.attachments.get` unnecessary because the attachment bytes are already in the MIME
      we hold. Parse locally with `mail-parser`.
- [x] 4.4 — Incremental sync via `history.list` (2 units per page, so idle polling is essentially
      free). **Cursor discipline, which is where this goes wrong:** page through the whole chain,
      commit only after the last page, and prefer `max(History.id)` actually processed, falling back
      to the response's top-level `historyId` only when `history[]` came back empty. The top-level
      `historyId` is the mailbox head *at request time*, not the last record on the page — storing
      it mid-pagination loses changes.
- [x] 4.5 — Run **one unfiltered history stream per account**; never pass `labelId`. A filtered
      cursor advances more slowly than the mailbox, so a quiet label goes stale and then expires.
      Filter locally.
- [x] 4.6 — **HTTP 404 from `history.list` is a first-class state, not an error.** Google documents
      history as valid "typically at least a week" but "in some rare circumstances may be valid for
      only a few hours" — a laptop closed over a weekend is inside the documented failure envelope.
      A 404 can also arrive mid-pagination as the window slides. Both abandon the chain without
      committing and schedule a full resync. Match on the body's `error.errors[].reason` too, since
      404 is otherwise indistinguishable from an ordinary not-found.
- [ ] 4.7 — Apply history record types correctly: `messagesAdded`, `messagesDeleted` (permanent
      expunge only — **not** trash), `labelsAdded`, `labelsRemoved`. Trash and untrash arrive as
      label changes of `TRASH`. Label-change records carry `labelIds` inline, so they need no
      follow-up fetch at all.
- [x] 4.8 — Rate-limit handling against the **current** (post-2026-05-01) quota model: 6,000
      units/min/user, 1.2M/min/project, and an 80M/day project ceiling that cannot be raised.
      Retryable is 403 with `domain == "usageLimits"`, 429, and 5xx; 403 `domainPolicy` is terminal
      and means a Workspace admin blocked the app, which needs its own message. Also handle the
      undocumented per-user **concurrent request** 429 — Snail competes with the user's phone for
      it — with the adaptive limiter from E16.4.
- [x] 4.9 — Enable gzip properly: `Accept-Encoding` **and** a User-Agent containing the literal
      string `gzip` (e.g. `snail/0.1 (gzip)`). Google silently disables compression without the
      second half, which is easy to miss and expensive on a raw-MIME backfill.

**iCloud (IMAP):**
- [ ] 4.10 — IMAP client on `async-imap` (tokio feature; the default is async-std). Chosen over the
      `imap` crate, which is on a 19-month-old alpha and whose README asks for maintainers.
- [ ] 4.11 — **XOAUTH2/OAUTHBEARER is not needed here** (iCloud uses app passwords), but note for
      the record that no IMAP crate ships either mechanism — they expose a generic `Authenticator`
      trait. Irrelevant for iCloud; relevant if Gmail-over-IMAP ever becomes the fallback from 3.2.
- [ ] 4.12 — **Re-issue `CAPABILITY` after authentication.** iCloud advertises only
      `XAPPLEPUSHSERVICE IMAP4 IMAP4rev1 SASL-IR AUTH=ATOKEN AUTH=PLAIN` in the pre-auth greeting,
      and only reveals `IDLE`, `CONDSTORE`, `QRESYNC`, `UIDPLUS`, `NAMESPACE` and the rest *after*
      login. A client that caches the greeting concludes iCloud has no IDLE and silently falls back
      to polling — this is exactly the bug Thunderbird shipped until version 80. Build it in from
      the first connection.
- [ ] 4.13 — Incremental sync via CONDSTORE/QRESYNC, with two independent problems to solve:
      **(a) the server side** — verified 2026-09-21 (E0.4): `ENABLE CONDSTORE` **and**
      `SELECT INBOX (CONDSTORE)` both return OK, so the parameter form is available and the old
      `BAD Unexpected extra arguments` claim is stale; **(b) the client side** — no maintained async
      Rust IMAP crate implements QRESYNC at all (`async-imap` has zero support; the sync `imap`
      crate parses the responses but cannot issue the commands). `async-imap` does expose a raw
      escape hatch (`Session::run_command` + `read_response`), confirmed in E0.4. Either issue raw
      commands for QRESYNC, or accept CONDSTORE-only and detect deletions by periodic UID
      resynchronization. **Decision still needed** — the escape hatch exists, so QRESYNC is on the
      table.
- [ ] 4.14 — `UIDVALIDITY` change = the mailbox identity changed = discard every cached UID for it
      and resync. Rare, catastrophic if mishandled, trivial if handled.
- [ ] 4.15 — IMAP IDLE for push on the selected mailbox, with a periodic re-issue (servers drop
      idle connections) and a polling fallback. Idle only covers one mailbox per connection, so
      Inbox gets IDLE and everything else gets polled.
- [ ] 4.16 — Connection pooling with a low ceiling and a reconnect/backoff policy; servers count
      connections and get unhappy.
- [ ] 4.17 — SMTP send via `mail-send` (Apache-2.0/MIT, and it supports XOAUTH2 **and**
      OAUTHBEARER, which `lettre` does not) over implicit TLS on 465 or STARTTLS on 587. Note that
      IMAP does not normally file the sent copy for you, so append to `Sent Messages` explicitly —
      **but verify first whether iCloud already files it server-side** (E0.4), because doing both
      gives the user duplicate sent mail. Gmail's REST `messages.send` files it automatically. This
      is a real behavioural difference the trait must hide.
- [x] 4.18 — Folder mapping **by name, not by flag**: verified 2026-09-21 (E0.4), iCloud **does**
      advertise SPECIAL-USE on two of the five — `Sent Messages [\Sent]` and `Deleted Messages
      [\Trash]` — so flag discovery covers those, but `Archive`, `Junk`, `Notes` and `Drafts` carry
      no attribute and need name mapping. iCloud's names are also non-standard — `Sent Messages` and
      `Deleted Messages`, not `Sent` and `Trash`; INBOX is `[\NoInferiors]`. Query `NAMESPACE` at
      runtime for the delimiter and prefix rather than guessing, map onto the handoff's five fixed
      mailboxes, and expose a user-editable override.
- [ ] 4.19 — iCloud also lacks **MOVE** (RFC 6851), so a move is `COPY` + `STORE \Deleted` +
      `UID EXPUNGE`, which UIDPLUS makes safe. And it lacks **COMPRESS=DEFLATE**, so a backfill is
      uncompressed on the wire — budget accordingly.
- [ ] 4.20 — iCloud limits, enforced client-side so the user gets a useful message instead of a
      server rejection: **1,000 messages/day, 1,000 recipients/day, 500 recipients/message, 20 MB
      per message**. Apple does not document the SMTP status code for quota exceedance — capture it
      empirically (E0.4) so "over quota, retry tomorrow" is distinguishable from a permanent
      failure. **Never retry a quota rejection in a loop**; Apple's terms allow suspension for load
      caused *unintentionally*.
- [ ] 4.21 — Conservative connection pool (default 2–4). Apple documents no IMAP connection limit;
      the widely-cited ~5 figure has no primary source. Treat connection refusal at the greeting as
      a distinct, backoff-triggering error.

**Both:**
- [x] 4.22 — MIME parsing with `mail-parser` (Apache-2.0/MIT, actively maintained, zero-copy, and
      it implements RFC 8621 §4.1.4 body-part selection — exactly the "give me the display body"
      logic this app needs). Charset decoding through `encoding_rs`/`charset` for the legacy
      encodings real mail still carries.
- [ ] 4.23 — Sync is **idempotent and crash-safe**: killing the app mid-sync re-runs from the last
      committed cursor and converges. Tested by actually killing it, repeatedly, in CI.
- [ ] 4.24 — Full-resync path that preserves local-only state (pending ops, read state not yet
      pushed, PGP verification results) rather than wiping the account.

### E5 — Mail: sidebar, list and reading pane
*Screen 1a. Three panes: `sidebar 232 | list 336 | reading flex`.*

- [ ] 5.1 — Message list as a single `uniform_list` over the current mailbox's ordered ids, with
      `UniformListScrollHandle`. Rows `w_full()` (§1.2). Row height is fixed by the handoff's
      three-line layout (7px dot column, sender+timestamp, subject, 2-line clamped preview) —
      measure it once and keep every row identical so one `uniform_list` serves the whole list.
- [ ] 5.2 — Row states exactly per handoff: unread (7px `#2c5fb8` dot, sender 13/600 `#1b1917`,
      subject 12.5/500, preview `#6f6963`); read (**7px-wide empty spacer keeps text aligned**,
      sender 13/500 `#3f3a35`, preview `#8a837b`); selected (`#e2eaf7` + `inset 3px 0 0 #2c5fb8`);
      hover `rgba(0,0,0,.03)`. Plus the undesigned states from E1.9: focused-not-selected,
      multi-selected, pending-operation, and send-failed.
- [ ] 5.3 — Preview text: 2-line clamp. GPUI has no `-webkit-line-clamp`, so `snail-ui` computes the
      truncation from measured text width and the two-line box, tested against fixtures.
- [ ] 5.4 — Sidebar: section label `MAILBOXES` (DM Mono 10/600, .09em, `#a09890`, via E1.5), five
      fixed mailboxes with icons and unread counts, 20px spacer, `ACCOUNTS` with per-account dot
      colours. Selected item `#d7e2f4` fill, 600 weight, icon and count in accent.
- [ ] 5.5 — Per-mailbox list header: name 14/600 + "N unread" 11/400, bottom rule.
- [ ] 5.6 — Reading pane: subject 19/600/-0.01em, sender row (32px avatar, name, "to me · 9:14 AM",
      Reply/Forward outlined buttons), body in **Newsreader 15/1.7 `#241f1b`**, attachment chips.
      Avatars are generated initials on tinted circles — two palettes from the handoff, extended to
      a deterministic hash→palette function so every correspondent gets a stable colour.
- [ ] 5.7 — Attachment chips: icon by MIME type, filename, human size. Click opens with the system
      opener; download-to-Downloads via the native save dialog (portal on Linux, §1.2).
- [ ] 5.8 — Selection behaviour in `snail-ui` and unit-tested there: `↑`/`↓` move, `Shift` extends,
      `⌘`/`Ctrl` toggles, selection survives a sync that inserts rows above it, and deleting the
      selection moves to the next row (not the top).
- [ ] 5.9 — Auto-mark-read after ~1s dwell on a selected message (handoff behaviour), cancelled if
      the selection moves first. Decrements the mailbox count optimistically.
- [ ] 5.10 — Toolbar: New message, archive, trash, reply icons, search field. Icons disabled and
      dimmed when there is no selection — an E1.9 state neither mockup draws.
- [ ] 5.11 — Responsive collapse: below ~900px window width the list collapses to the 1b layout
      (handoff rule). Above it, panes are fixed-width with only the reading pane flexing.
- [ ] 5.12 — Budget check against the E0.9 fixture: 200k-message mailbox, scroll at 4000 px/s,
      draw p50 < 2 ms (§4).

### E6 — The HTML message renderer
*The one genuinely hard piece of this plan, and the reason Snail can stay small. Scoped by the
E0.3 spike's failure taxonomy. All of it lives in `snail-core` (parse/sanitize) and `snail-ui`
(layout), so the whole thing is testable without GPUI.*

- [ ] 6.1 — MIME → displayable document: walk the part tree, pick the best alternative
      (`text/plain` vs `text/html` per the settings toggle), resolve `multipart/related` `cid:`
      references to cached attachment files, handle `format=flowed` for plain text, and decode
      every charset the wild throws (not just UTF-8 — legacy `ISO-8859-*`, `Shift_JIS`, `GB2312`).
      **Carry the E0.3 selector finding:** `body_html()` converts plain text and the
      `html_body_count()`/`text_body_count()` lists are unreliable (mail-parser copies a lone part
      across both), so select on `part.is_text_html()` / `is_text()`.
- [ ] 6.2 — Sanitize with `ammonia` against an explicit allowlist. Strip `<script>`, `<iframe>`,
      `<object>`, `<form>`, every `on*` handler, and `javascript:`/`data:` URLs. **All remote URLs
      are rewritten to a blocked placeholder by default** — this is architecture, not a setting
      (§1.4). A per-sender "always load images" allowance is stored locally.
      **E0.3 note:** ammonia's default *strips the `style` attribute and `<style>` content*, so the
      CSS subset 6.3/6.4 needs must be extracted **before** (or with a filtered
      `filter_style_properties`) ammonia's pass — otherwise the pipeline loses the formatting it is
      about to implement. Also, `TextView::html` fetches remote images on its own, so URL rewriting
      must happen before the document reaches any renderer.
- [ ] 6.3 — Parse the sanitized document with `html5ever` into our own simplified DOM — elements,
      text, and the computed subset of style. Not a general CSSOM: resolve `style=""`, the
      supported properties from `<style>` blocks, and the legacy presentational attributes email
      actually uses (`bgcolor`, `align`, `valign`, `width`, `height`, `cellpadding`, `cellspacing`,
      `border`, `<font size|color|face>`).
- [ ] 6.4 — The supported CSS subset, written down as a document and as a test fixture:
      `color`, `background-color`, `font-family|size|weight|style`, `text-align`,
      `text-decoration`, `line-height`, `margin`, `padding`, `border*`, `width`/`height`/`max-width`
      (px and %), `display: block|inline|inline-block|table*|none`, `vertical-align`, `list-style`.
      **Not supported and deliberately so:** float, position, flex, grid, transforms, media
      queries, pseudo-elements, web fonts. Anything unsupported is dropped, never approximated.
- [ ] 6.5 — Layout engine in `snail-ui`: block and inline formatting over the subset, producing
      positioned boxes from a `FontMetrics` trait (so tests use a fake metrics impl and GPUI
      supplies the real one). Inline layout must handle line breaking, whitespace collapsing, and
      mixed font runs.
- [ ] 6.6 — **Table layout.** The single biggest piece of E6, because email layout *is* tables.
      Fixed and auto table algorithms, `colspan`/`rowspan`, nested tables to the depth real
      newsletters use. Budget it as its own multi-week story; fixtures come from E0.3.
- [ ] 6.7 — Paint the positioned boxes as GPUI elements: text runs, background quads, borders,
      images from the cache. Virtualize by only building elements for boxes intersecting the
      viewport, so a 40-screen newsletter costs one screen of elements.
- [ ] 6.8 — Inline images: `cid:` parts from the cache, and remote images once unblocked — fetched
      on the background executor, decoded with `image`, cached content-addressed, with a
      reserved-size placeholder so unblocking doesn't reflow the world.
- [ ] 6.9 — Quoted-text folding: detect `<blockquote>` chains and `>`-prefixed plain text, collapse
      everything below the first quote boundary behind a "•••" control. This is what makes a
      20-message reply chain readable and is absent from both handoffs.
- [ ] 6.10 — Text selection and copy across the rendered document. Non-trivial with custom layout
      and genuinely expected by users; scoped explicitly so it is not discovered late.
- [ ] 6.11 — Link handling: hover shows the real target, click asks before opening anything whose
      visible text disagrees with its href (the phishing case), opens via the system browser.
- [ ] 6.12 — Escape hatch: "Open in browser" writes the sanitized document to a temp file and hands
      it to the system browser. The honest answer for the 2% of messages the subset can't do.
- [ ] 6.13 — Fallback rendering: if layout fails or exceeds a node/time budget, fall back to
      `text/plain`, then to a flattened text extraction. **A message must always render something.**
- [ ] 6.13b — Before building 6.5–6.7, spend a day on **gpui-kit's `TextView`**, which already
      renders Markdown *and* HTML natively (there is an `example-html` in its gallery). If its HTML
      support covers a useful fraction of the E0.3 corpus, it is either a shortcut to a working
      reading pane or, at minimum, the fallback renderer for 6.13. Nobody should write a layout
      engine without first checking what the component library already does.
      **Answered by E0.3 (2026-09-21): it is not a shortcut, but it is not wasted.** `TextView::html`
      exists at 0.6.4 (`component::text::TextView::html(id, text)`, `.selectable()`,
      `.scrollable()`, `.table_actions()`, `.on_link_click()`), but it is a Markdown-grade subset:
      it honors only `color`, `background-color`, `width`, `height` from an inline `style`, has no
      font-family/size/weight (outside tags), alignment, margin, padding, border or line-height, and
      drops `<style>` blocks. Microsoft-grade mail therefore renders unformatted.
      **Conclusion: E6.5–E6.7 are required, not optional; use `TextView` as 6.13's always-renders
      fallback.** Also note it fetches remote images itself, so 6.2's URL rewriting is mandatory
      *before* any renderer.
- [ ] 6.14 — Corpus test: render the full E0.3 corpus plus everything added since, snapshot the box
      trees, and fail CI on an unexplained diff. This is the regression net for the whole epic.

### E7 — Compose, reply and send
*Screen 1c, 620×520, plus the inline reply box in 1b.*

- [ ] 7.1 — Compose window as a second GPUI window (not a modal) so it survives navigation. From
      picker, To/Cc/Bcc, Subject, body, format bar, Send. Built on whatever E0.2 concluded about
      `Textarea`.
- [ ] 7.2 — Recipient tokens per handoff: 18px avatar + name pill, r20, `#e2eaf7`/`#1d4489`,
      backspace deletes the last token, click selects it, invalid addresses get an error state
      (undesigned — E1.9).
- [ ] 7.3 — Address autocomplete from a local contacts table built by harvesting From/To/Cc of
      every synced message with a frequency-and-recency score. **No contacts API, no network.**
- [ ] 7.4 — Reply / Reply-all / Forward: correct `In-Reply-To` and `References`, recipient
      derivation (including `Reply-To` and list headers), attribution line, quoted body, and
      forwarded-message attachment handling.
- [ ] 7.5 — Inline reply box in the thread view (handoff 1b): collapsed placeholder that expands in
      place, promotable to the full compose window without losing the draft.
- [ ] 7.6 — Draft autosave to the local store on a debounce, with the handoff's "Draft saved"
      indicator; drafts survive a crash and appear in the Drafts mailbox. **Remote draft sync is
      explicitly out of scope for v1** — local drafts only, stated so the gap is deliberate.
- [ ] 7.7 — Attachments: add via portal/native picker, drag-and-drop onto the compose window, size
      warning above the provider limit, correct `multipart/mixed` assembly.
- [ ] 7.8 — Body format: plain text by default with a minimal HTML mode behind the handoff's format
      bar (bold, italic, list, link). **The renderer of our own HTML is E6's; keep the generated
      markup trivially simple** so replies are readable in every other client.
- [ ] 7.9 — Signature from settings, serif per the handoff, inserted above the quote on reply.
- [ ] 7.10 — Undo send: configurable window (default 10s), the message parked in `pending_op` with
      a visible countdown and Undo, dispatched only when it expires. Nothing leaves the machine
      during the window.
- [ ] 7.11 — Send path per provider behind the `MailProvider` trait, with the sent copy landing in
      the right mailbox for each (the two providers differ here — see §6).
- [ ] 7.12 — Send failure handling: the message stays in the queue, the row shows a failed state,
      and retry is explicit. Never silently drop a send.

### E8 — Threading, triage and undo

- [ ] 8.1 — Threading in `snail-ui`, pure and unit-tested: JWZ-style `References`/`In-Reply-To`
      linking with subject-based fallback, reconciled with Gmail's server-side `threadId` where it
      exists. **The two must agree on one thread id** so a thread doesn't split when half its
      messages come from Gmail and half from iCloud.
- [ ] 8.2 — Thread view (screen 1b): thread subject 24/600, mailbox pill, participant list, message
      count, collapsed rows (`#f6f3ee`, 28px avatar, name, snippet, date), newest expanded by
      default, click to expand in place.
- [ ] 8.3 — "Group messages by thread" setting genuinely off: the list shows individual messages.
      Both paths must be equally fast — this is a store query shape, not a UI filter.
- [ ] 8.4 — Triage actions — archive, trash, mark read/unread, move — applied optimistically to the
      store and UI, enqueued in `pending_op`, with a visible rollback on failure.
- [ ] 8.5 — Undo affordance per handoff: a transient bar after archive/trash with a real inverse
      operation (not just a UI restore — it must undo the queued remote op too, or issue the
      inverse if it already ran).
- [ ] 8.6 — Gmail labels vs IMAP folders reconciled into one `label` concept: Gmail's many labels
      per message and iCloud's one folder per message both project onto the handoff's five fixed
      mailboxes plus an account-specific remainder. The mapping table is the interesting part —
      Gmail's archive is "remove INBOX", iCloud's is "move to Archive".
- [ ] 8.7 — Bulk actions over a multi-selection, with one undo covering the batch.
- [ ] 8.8 — Conversation-level actions (archive whole thread) distinct from message-level.

### E9 — Search

- [ ] 9.1 — FTS5 virtual table over subject, sender, recipients and body text, kept in sync by
      triggers in the same transaction as the message write.
- [ ] 9.2 — Query language in `snail-ui`, parsed and unit-tested: bare terms, quoted phrases,
      `from:`, `to:`, `subject:`, `has:attachment`, `is:unread`, `in:mailbox`, `before:`/`after:`,
      and date words ("yesterday", "last week"). Falls back to a plain term on a parse error rather
      than erroring.
- [ ] 9.3 — Search runs on the background executor with a generation guard; results stream into a
      grouped list. Budget: first results under 50 ms on the 200k fixture (§4).
- [ ] 9.4 — Grouped results in one `uniform_list` by normalizing group-header rows to the same
      height as result rows — the trick Ferrite used for its grouped search screen.
- [ ] 9.5 — Search is local-only and works offline. **No provider search API** — the local index is
      always authoritative, which is also what makes it instant.
- [ ] 9.6 — `⌘F` focuses search; Escape clears and restores the previous list; the query survives
      switching mailboxes.

### E10 — PGP
*In v1 at the owner's request. Deliberately placed after the mail client works, because it is
orthogonal and its failure modes must never block reading ordinary mail.*

- [ ] 10.1 — Key handling: import existing secret and public keys from disk, list them, show
      fingerprints and expiry. **No key generation in v1** unless E10.2 is cheap — importing the
      key you already have is the real use case.
- [ ] 10.2 — Passphrase handling: prompt per session by default, optional keychain storage (E2.8),
      never written to disk in the clear, and zeroized after use.
- [ ] 10.3 — Verify signatures on inbound mail: PGP/MIME (`multipart/signed`,
      `application/pgp-signature`) and inline-PGP. A verified/unverified/failed indicator in the
      reading pane, designed as part of E1.9 since no handoff has one.
- [ ] 10.3b — **The certificate-semantics layer (§1.6).** rpgp gives cryptographic validity and
      stops there, so this story supplies what a mail client actually means by "valid signature":
      the signing key is not **expired**, not **revoked** (directly or via its primary), carries the
      **signing key flag**, and the binding signature chain back to the primary key checks out; plus
      an **algorithm policy** that refuses MD5 and SHA-1 signatures rather than reporting them as
      good. Either adopt `rpgpie` or implement against it as the reference. **Fixture-test with
      deliberately bad certificates** — revoked key, expired subkey, encryption-only subkey used to
      sign, SHA-1 signature — and assert each is reported as *invalid*, because every one of these
      is a real vulnerability if it silently passes. A "cryptographically valid but policy-rejected"
      signature is its own UI state, distinct from both valid and forged.
- [ ] 10.4 — Decrypt inbound `multipart/encrypted` and inline-PGP messages on the background
      executor, cached decrypted in memory only — **never written to the message cache or FTS
      index** (the index would otherwise leak plaintext to disk). Consequence stated plainly:
      encrypted mail is not searchable by body. That is the correct trade.
- [ ] 10.5 — Sign outbound: PGP/MIME `multipart/signed` per RFC 3156, with correct
      canonicalization — CRLF line endings, trailing-whitespace handling, the boundary and the
      `protocol="application/pgp-signature"` parameter. **No crate does this for either library**
      (§1.6), so it is ours, and it is the part every implementation gets wrong first.
      Fixture-test against GnuPG and Thunderbird output, round-tripping in both directions.
      **Delta Chat's `src/mimefactory.rs` is the one production RFC 3156 implementation on current
      rpgp** — in-tree application code rather than a library, but the reference worth reading.
      (`mml-lib` does implement RFC 3156, but its rpgp path is pinned to 0.10 from 2023: no v6, no
      SEIPDv2, and missing both 2024 CVE fixes. Not usable.)
- [ ] 10.6 — Encrypt outbound to recipients whose public keys are known; clear UI for "can't
      encrypt, key missing for X" *before* Send, not after.
- [ ] 10.7 — Key discovery: local keyring, keys attached to received mail, and WKD lookup. **No
      keyserver by default** (it leaks the social graph); an explicit opt-in per lookup.
- [ ] 10.7b — **WKD, written here, and written correctly.** rpgp has no WKD and has never been
      asked for it; the de-facto implementation is `sequoia-net`, which is LGPL and drags in OpenSSL
      via `hickory-resolver`, reintroducing exactly the C dependency §1.6 exists to avoid. So this
      is ours — about 250 lines. Write it **from the spec, not by copying the LGPL source.**

      The mechanics: lowercase the ASCII of the local part (non-ASCII unchanged), SHA-1 it,
      z-base-32 encode to a fixed 32 characters, and build one of two URLs —
      **advanced**, `https://openpgpkey.<domain>/.well-known/openpgpkey/<domain>/hu/<hash>?l=<local>`,
      or **direct**, `https://<domain>/.well-known/openpgpkey/hu/<hash>?l=<local>`.

      **The part everyone gets wrong, and the reason this is its own story:** the spec says to try
      advanced first and fall back to direct *only when the `openpgpkey.<domain>` sub-domain does
      not exist in DNS* — and states outright that **a non-responding server is not a reason to fall
      back**. So an HTTP 404 from the advanced URL must **not** trigger a direct-method retry; it is
      a definitive "no key". Every implementation surveyed gets this wrong in one direction or the
      other: `sequoia-net` and `pgp-lib` fall back only on transport errors (and `sequoia-net`'s own
      source carries a `// XXX` admitting it), while `wecanencrypt` falls back on any non-2xx.
      Follow the spec: the fallback trigger is a DNS existence check.

      Also: cap the response size, follow redirects with a bounded limit, and note that WKD is
      **still only an Internet-Draft** after 22 revisions and ~10 years (`-22`, 2026-07-22), with a
      2026 IETF early review of "Not ready". Treat it as a useful convention, not a stable standard,
      and never as an authoritative trust signal on its own.
- [ ] 10.8b — Two rpgp API sharp edges, fixed once in a wrapper so no call site meets them:
      `Message::verify()` **errors if the message has not been read to the end**, so verification
      must follow a full read; and `verify_nested()` returns a `VerificationResult::Invalid`
      **rather than an error** when a signature does not match — a silent failure that reads exactly
      like success if you only check the `Result`. Also: **never enable rpgp's `asm` feature** — it
      is the single path that pulls `cc` and a C toolchain, defeating the reason §1.6 chose this
      library. A `#[test]` asserting the built feature set catches that.
- [ ] 10.9 — **Security-watch chore, because tooling will not do it for you.** `cargo audit` and
      `cargo deny` run in CI, but §1.6 records that RustSec carries only one of rpgp's six
      advisories and that the currently-unpatched one is in no public database. So: subscribe to
      the repo's own security-advisories page, pin `pgp` exactly, and re-check on every upgrade.
      Mirror rpgp's own `deny.toml` ignores (RUSTSEC-2023-0071, the `rsa` Marvin attack) explicitly
      and **with a comment saying why**, rather than letting them sit as unexplained suppressions.
- [ ] 10.10 — Bound the decompression path ourselves rather than waiting for the unpatched
      advisory: cap decompressed size and nesting depth on inbound PGP, well below rpgp's 1 GiB
      default, and treat exceeding it as a malformed message.
- [ ] 10.8 — Crypto never touches the UI thread, and a malformed or hostile PGP payload degrades to
      "could not decrypt" rather than hanging or crashing. **Parse untrusted PGP behind a
      catch-unwind boundary** — this is not defensive paranoia, it is a documented, still-live bug
      class in both candidate libraries (§1.6). Fuzz the parsing path (E17.5).

### E11 — Calendar sync
*Same shape as E4: two providers, one `CalendarProvider` trait, no view ever knows which is which.*

**Google Calendar:**
- [ ] 11.1 — `calendarList.list` with its own sync token to discover calendars, colours, `selected`
      and `accessRole`. Note that `calendarList` is the user's *subscription* list with per-user
      presentation state, distinct from the underlying calendar — a calendar removed from it still
      exists, so do not delete its events on that signal alone.
- [ ] 11.2 — Per-calendar `events.list` with a **per-calendar `syncToken`**. Sync tokens are
      per-collection; there is no single account-wide cursor.
- [ ] 11.3 — **`nextSyncToken` appears only on the very last page.** Intermediate pages carry only
      `nextPageToken`. Commit the token once, at the end — the same cursor discipline as E4.4.
- [ ] 11.4 — **Sync masters, not instances: `singleEvents=false`, and expand RRULE locally.** This
      is the load-bearing decision of the epic. `timeMin`/`timeMax` are *forbidden* alongside a
      `syncToken`, so `singleEvents=true` would mean an unbounded expansion — an open-ended weekly
      meeting expands forever, with no horizon to stop at. Syncing masters keeps pagination bounded
      by real event count, and local expansion (E13.4) gives offline range queries for free.
- [ ] 11.5 — Keep **every** query parameter identical between the initial and incremental requests.
      The forbidden-with-`syncToken` set is exactly `iCalUID`, `orderBy`, `privateExtendedProperty`,
      `q`, `sharedExtendedProperty`, `timeMin`, `timeMax`, `updatedMin`, and `showDeleted` cannot
      be false. Violating any of these is a 400. Sort locally, since `orderBy` is unavailable.
- [ ] 11.6 — Deletions arrive as `status: "cancelled"`, and the two kinds must not be conflated:
      a cancelled *exception of an uncancelled recurring event* means "hide this instance"; any
      other cancelled event means "the event was deleted, remove your copy." Getting this backwards
      either leaves deleted events on screen or wipes live series. Cancelled exceptions are only
      guaranteed to carry `id`, `recurringEventId` and `originalStartTime`, so the deserializer
      must tolerate a nearly empty event.
- [ ] 11.7 — **410 `fullSyncRequired` is routine, not an error.** Google documents no TTL for sync
      tokens and invalidates them for reasons including ACL changes on any subscribed calendar, so
      a sharing change can force a resync at any moment. Wipe that calendar's events and re-sync
      it — that calendar only, not the account.
- [ ] 11.8 — Key on `recurringEventId` + `originalStartTime` for instances, never on start time —
      `originalStartTime` is explicitly the stable identity "even if the instance was moved."
      Persist `iCalUID` too as the cross-system dedupe key against CalDAV.
- [ ] 11.9 — Do **not** use the `updated` field as a change detector: Google states that updating
      an event's reminders does not change it.
- [ ] 11.10 — No `events.watch`. Calendar push requires an HTTPS endpoint with a CA-signed
      certificate and has no pull alternative — strictly unusable without a server. Poll.
- [ ] 11.11 — Calendar quota is counted in **requests**, not Gmail's weighted units: 600/min/user,
      10,000/min/project, 1,000,000/day/project. A sync-token poll is one request per calendar, so
      cadence × calendar count is the thing to watch.

**iCloud CalDAV:**
- [ ] 11.12 — Discovery: `/.well-known/caldav` → `current-user-principal` → `calendar-home-set` →
      PROPFIND Depth:1 for `displayname`, `resourcetype`, `supported-calendar-component-set`,
      `getctag`, `calendar-color`, `current-user-privilege-set`. Two iCloud-specific traps:
      **(a) do not validate that the response path matches the request path** — iCloud answers
      `/.well-known/caldav` with a principal path like `/8034509913/principal/` and strict clients
      reject it; **(b) follow the 301/302 redirect onto the sharded host
      `pNN-caldav.icloud.com`** and keep talking to it, but re-resolve from `caldav.icloud.com` on
      any unexpected 404 rather than persisting the shard hostname forever.
- [ ] 11.13 — **Implement both sync paths and probe which to use.** Whether iCloud supports RFC 6578
      `sync-collection` is genuinely unestablished — `libdav` (the only credible CalDAV client crate,
      ISC) detects the capability but **does not implement incremental sync at all**, so this is our
      code either way. Read `DAV:supported-report-set` per collection at setup: use `sync-collection`
      if present; otherwise poll the cheap `getctag` PROPFIND per collection and, on a change, diff
      an etag listing and fetch with `calendar-multiget`. **A ten-minute live test settles this —
      do it in E0.4.**
- [ ] 11.14 — **Never synthesize resource hrefs from UIDs.** Any collection another client has
      written contains resources whose filename ≠ the VEVENT UID; constructing the URL from the UID
      addresses a non-existent resource, the ETag comes back empty, `If-Match` is omitted, and
      lost-update protection silently fails. Always use the href the server gave you. Also filter
      the collection's own href out of multistatus responses — iCloud includes it, and naive
      parsers treat it as an extra event.
- [ ] 11.15 — Never assume the object you wrote is the object stored: Apple's server normalizes
      uploaded iCalendar data and, when it does, returns **no ETag** on the PUT. Follow with a GET
      to obtain the canonical object and its real ETag. Change detection compares server state to
      last-known-server state, never to our own serialization.
- [ ] 11.16 — Bounded time-range queries: inherited CalendarServer behaviour rejects far-past and
      far-future ranges with **403 max-date-time**, which breaks naive "sync everything" loops.
- [ ] 11.17 — Rate limiting: iCloud returns **503 "Rate Limit Exceeded"** with no published
      thresholds and no way to request an increase. Treat 403/429/503 as transient with exponential
      backoff and honour `Retry-After`; treat **401 as credential-revoked** (almost always the
      app-specific password died with a password change, per E3.4).
- [ ] 11.18 — No push. Neither DAVx⁵, vdirsyncer nor Thunderbird uses push against iCloud, and the
      APNs transport needs an Apple-issued certificate that a cross-platform client cannot have.
      Poll `getctag`, which is one cheap PROPFIND per collection.
- [ ] 11.19 — Calendar creation is awkward on iCloud (minimum name lengths, and calendars created
      by third-party clients can behave oddly). v1 **reads and writes events in existing calendars
      and does not create calendars**; the user makes them in Apple's UI. Stated as a limit, not a
      bug.

**Both:**
- [ ] 11.20 — iCalendar parsing with **`calcard`** (Apache-2.0/MIT, actively maintained). Chosen
      over `icalendar` + `rrule` for one decisive reason: it is the only Rust crate that handles
      **VTIMEZONE properly** — it has a real `TzResolver` that reads VTIMEZONE components and
      resolves `TZID`, *and* a Windows/Exchange TZID → IANA mapping table ("Pacific Standard Time"
      → `America/Los_Angeles`). `icalendar` does not model VTIMEZONE at all and hands back the TZID
      as an unresolved string; `rrule` has had no commit in 17 months. Real calendars are full of
      Exchange-originated invitations, so this is not a theoretical concern. Cost: `calcard` is
      0.3.x and moving fast — pin exactly, like gpui-kit.
- [ ] 11.21 — Write path: local edit → optimistic store write → `pending_op` → provider. Google
      takes a patch; CalDAV takes a whole-object PUT with `If-Match` on the etag, and a 412 means
      someone else won — refetch, and surface a real conflict rather than clobbering.
- [ ] 11.22 — Poll cadence with jitter (Google's docs explicitly call a synchronized-midnight full
      sync "a common bad practice" and suggest ±25%), 5–15 min foregrounded, much slower on battery.

### E12 — Calendar views
*Calendar handoff screens 01–05, restated in the mail design language per §1.3. All geometry
survives; only colour, type and radius change.*

- [ ] 12.1 — Grid geometry in `snail-ui`, pure and unit-tested, because this is where calendars
      actually go wrong: month cell → date mapping across a 6×7 grid and month boundaries; time-grid
      `top = (start_hour − 7) × pitch` and `height = duration × pitch − gap` at the three pitches
      (45 / 47 / 49px); all-day band assignment; agenda day grouping. Tested against DST
      transitions in both directions and across the year boundary.
- [ ] 12.2 — **Overlapping events**, undesigned in the handoff because the sample data never
      overlaps. Column-packing algorithm (group overlapping events, assign columns, width =
      pane/columns) in `snail-ui` with fixtures for the cases that break naive implementations:
      three-way overlap, an all-day-length event beside short ones, and events that only touch at
      an endpoint (which must *not* count as overlapping).
- [ ] 12.3 — Month view, built on Taffy's **CSS Grid** support (§1.2) rather than nested flex:
      88px header (title 42/500/-0.03em + year in DM Mono + week number +
      calendar count + date stepper), 30px weekday strip, 6×7 grid. Date pill states (today =
      solid accent; normal; out-of-month), day tags, max 3 event chips then "+N more".
- [ ] 12.4 — Week view: 78px header, 52px column header, 40px all-day band, 15 hour rows at 45px.
      Today column washed and accent-coloured. All-day chips in the band.
- [ ] 12.5 — 3-day and Day views: the same engine at 47px and 49px pitch, with the Day view's
      300px right rail (DUE TODAY / REMINDERS / REPEATS sections, dashed "Add to this day" footer).
      Note the handoff's own inconsistency — the Day now-line is hardcoded at 301px while week and
      3-day derive theirs from 13:40. **Derive all three from one clock.**
- [ ] 12.6 — Now-line: 1.5px accent rule with an 8px dot, plus the 3-day view's time flag. Updated
      on the minute boundary via `cx.notify()` + `cx.on_next_frame` called *from* render (§1.2) —
      never `request_animation_frame` from a callback, and never a 60 Hz timer for something that
      moves once a minute.
- [ ] 12.7 — Agenda view: 78px header, day groups with a 132px date block, zebra rows, and the
      events/events+tasks segmented control.
- [ ] 12.8 — Scrolling and virtualization, absent from the handoff (grids are cropped 07:00–21:00
      in the mock): time grids scroll the full 24 hours and **open scrolled to the current hour**;
      the agenda virtualizes over an effectively unbounded date range in both directions.
- [ ] 12.9 — Range loading: fetch expanded occurrences for the visible range **plus one range of
      buffer each side** (handoff instruction), on the background executor with a generation guard.
- [ ] 12.10 — Mini-month in the sidebar: 6×7 grid at 27px, out-of-month / in-month / selected
      (`#e2eaf7`) / today (solid `#2c5fb8`) states, month arrows.
- [ ] 12.11 — Calendar list in the sidebar with per-calendar swatch, name, event count, and
      visibility toggle; toggling drops the calendar out of every view immediately.
- [ ] 12.12 — View switching (Month / Week / 3-day / Day / Agenda) via the titlebar segmented
      control, the date stepper (‹ / Today / ›) and the mini-month, all driving one `focusedDate`.
- [ ] 12.13 — **Timezones.** The handoff has no timezone UI at all, but `tz` is on its Event entity
      and a calendar that gets this wrong is worthless. Events store an IANA zone; the grid renders
      in the local zone; an event in a different zone shows its original time as secondary text.
      Fixtures for: an event created in another zone, a DST-spanning recurring event, and a
      floating all-day event.
- [ ] 12.14 — Drag to create, drag to move, and edge-resize on the time grids — all undesigned, all
      expected. Typed `on_drag` payloads with snap-to-15-minutes and a live preview block.

### E13 — Event editor, recurrence and reminders
*Calendar handoff screen 06: a 576px sheet over a dimmed view, scrim inset below the titlebar.*

- [ ] 13.1 — The sheet: head (kicker, title field with the accent underline focus state), body
      (92px label column + controls), foot (Delete / Cancel / Save with ⌘↵). Scrim
      `rgba(0,0,0,.34)` inset `52px 0 0 0` so the titlebar stays live.
- [ ] 13.2 — Fields: title, when (date + time + all-day toggle), calendar picker, location,
      repeat, reminders, notes (Newsreader per §1.3). Built on gpui-kit `Input`, so gated on E0.2 —
      and note gpui-kit also ships `DatePicker` and `Calendar` components, which may serve the date
      field and the sidebar mini-month (E12.10) directly. Evaluate before hand-rolling either.
- [ ] 13.3 — Recurrence UI: frequency dropdown, weekly day multi-select (the 34×30 buttons),
      end-date field, and the **live occurrence count** ("54 occurrences") — which means the
      expander must run on every edit, fast.
- [ ] 13.4 — RFC 5545 recurrence expansion in `snail-core`, the single most test-worthy piece of
      the calendar. RRULE/RDATE/EXDATE, `COUNT` vs `UNTIL`, `BYDAY`/`BYMONTHDAY`/`BYSETPOS`,
      DST-crossing series, and exception instances that were moved rather than cancelled.
      **Google forbids `DTSTART`/`DTEND` inside its `recurrence[]` field** (§6), so the expander
      takes DTSTART separately — a real gotcha if an off-the-shelf crate expects them inline.
- [ ] 13.5 — Edit and delete **scope: this / this-and-future / all** — the handoff calls for it and
      it is where correctness lives. "This" creates an exception; "this and future" splits the
      series with a new master; "all" edits the master. Fixture-tested against what Google and
      CalDAV each actually do with the result.
- [ ] 13.6 — Reminders: multiple per event, relative ("10 min before") and absolute ("at 08:00
      same day"), as removable chips plus a dashed add button.
- [ ] 13.7 — **Reminders fire from the backend**, per the handoff, so they work with the window
      closed — a scheduler in `snail-services` holding the next N due reminders, resilient to
      sleep/wake (a laptop that was closed through a reminder should fire it late, once, not
      silently drop it or fire twenty).
- [ ] 13.8 — Attendees and RSVP. **Not in either handoff** — the calendar handoff explicitly has no
      attendee UI — but an invite you cannot answer makes the calendar read-only in practice, and
      invites arrive as mail, which Snail already has. Scoped as: show attendee list and status,
      accept/decline/maybe from both the event and the mail message. Needs a design pass first.
      **Gate iCloud RSVP on a probe**: whether iCloud honours RFC 6638 server-side scheduling is
      unverified, so probe `schedule-inbox-URL`/`schedule-outbox-URL` on the principal at setup and
      only enable the feature if they are present. Test what actually happens when an event with
      `ATTENDEE` properties is PUT — **double-sending invitations, once server-side and once by us,
      is the bad failure mode here.** A client-side iMIP fallback (what Thunderbird does) is the
      answer if the server does not schedule.
- [ ] 13.9 — Tasks: the sidebar TASKS section and Day rail cards, with open / overdue / done
      states, inline add, and due dates. **Local-only, and this is now a finding rather than a
      choice: Apple disabled Reminders sync over CalDAV** — modern iCloud Reminders ride a private
      protocol, and third-party clients cannot reach them at all. Google Tasks has its own separate
      API, out of v1 scope. So Snail's tasks are its own, stored locally, and the UI must not imply
      they sync anywhere.

### E14 — Settings and accounts

- [ ] 14.1 — Settings surface. The two handoffs disagree: mail uses a 620×600 window with three
      flat sections and no tabs; the calendar uses an in-window 236px nav with seven panes. **Take
      the calendar's in-window nav** (it scales, and a separate window is one more thing to manage)
      with the mail handoff's card-and-row visual treatment.
- [ ] 14.2 — Accounts pane: the calendar handoff's account cards — avatar with initials, name,
      detail line, status dot (synced `#0F766E` / syncing `#A16207` / offline `#8a837b` / error
      `#C2410C`), "Sync now", and per-calendar/per-mailbox toggle pills.
- [ ] 14.3 — Add-account flow: pick Google or iCloud, run the right auth path (E3), discover
      mailboxes and calendars, and let the user choose what syncs before the first full pull.
- [ ] 14.4 — General pane: the mail handoff's four rows — check interval, load remote images (off,
      with the privacy helper text), group by thread, undo-send window — plus the calendar's sync
      cadence (5 / 15 / 60 / manual) unified into one interval control per account.
- [ ] 14.5 — Signature pane, serif per the handoff, per account.
- [ ] 14.6 — PGP pane: keys, passphrase policy, default sign/encrypt behaviour, WKD opt-in.
- [ ] 14.7 — Keyboard pane listing every command and its binding, generated from the command table
      (E15) so it cannot drift.
- [ ] 14.8 — Advanced pane: store size and location, clear cache, re-index search, force full
      resync per account, open the log. **Force full resync is a real user-facing button**, not a
      hidden flag, because §6 guarantees it will be needed.
- [ ] 14.9 — Account removal: delete the local store rows and cache, revoke the token where the
      provider supports it, and wipe the keychain entry.

### E15 — Commands, keyboard and menus

- [ ] 15.1 — Command table in `snail-ui` (id, title, default binding, menu placement, key context,
      enable guard) — GPUI-free and unit-tested, following Ferrite's pattern.
- [ ] 15.2 — **One generic `RunCommand(CommandId)` action**, not one action type per command
      (§ Ferrite's actions.rs). The table projects into `Vec<KeyBinding>`, native macOS menus via
      `cx.set_menus`, and an in-window menu on Windows/Linux.
- [ ] 15.3 — Key contexts as an enum (Global / List / Reading / Compose / Calendar / Search /
      Editor) with the "don't fire while typing" guard expressed as the `!Input` context predicate.
      Critical here: a mail client is full of single-letter shortcuts that must not fire mid-compose.
- [ ] 15.4 — The handoffs' shortcuts, honoured exactly: `↑`/`↓` selection, `⌘R` reply, `⌘⇧D` send,
      `⌘⌫` trash, `⌘⇧A` archive, `⌘F` search, `⌘K` command/date search, `⌘↵` save event, `Esc`
      cancel. Platform glyphs rendered per OS.
- [ ] 15.5 — Focus discipline: a `FocusHandle` per focusable region, and the shell takes focus at
      launch and on window-focus-lost (with an open dialog winning) — otherwise GPUI dispatches
      shortcuts above the shell's handler and nothing works until you click. This exact bug is
      documented in the reference project; do not rediscover it.
- [ ] 15.6 — A test that constructs every binding, because `KeyBinding::new` panics on an
      unparseable keystroke and that should fail in CI, not at launch.
- [ ] 15.7 — `⌘K` palette: commands, mailboxes, calendars, and natural-language dates ("next
      tuesday", "aug 17") parsed in `snail-ui`.

### E16 — Sync orchestration, offline and notifications

- [ ] 16.1 — `SyncScheduler` in `snail-services`: per-account, per-collection cadence with
      **jitter** (§6 — Google's docs explicitly warn against synchronized midnight syncs), faster
      when the window is focused, slower backgrounded, slowest on battery, and a nudge on focus
      rather than a tighter timer.
- [ ] 16.2 — Network-state awareness: detect offline, stop retrying, resume promptly on reconnect,
      and never present offline as an error.
- [ ] 16.3 — Backoff on every provider: `min(2^n + jitter, 32–64s)`, distinguishing retryable
      (`usageLimits` 403, 429, 5xx) from terminal (auth failure, `domainPolicy`) — getting this
      wrong means either hammering Google or silently stopping.
- [ ] 16.4 — Adaptive concurrency per account, starting at 3–5 in flight and halving on a
      concurrency 429. Snail competes with the user's phone for the same undocumented per-user
      limit (§6).
- [ ] 16.5 — `pending_op` drain loop with idempotency keys, capped attempts, and a dead-letter
      state surfaced in the UI rather than an infinite silent retry.
- [ ] 16.6 — Sync status in the sidebar footer: dot, "N accounts synced", relative last-sync time,
      pending-change count, and a per-account breakdown on click.
- [ ] 16.7 — First-run experience: the window is interactive immediately, the first page of recent
      mail lands within seconds, and the backfill streams in behind it with visible progress. §6's
      arithmetic says a large mailbox takes **hours** to backfill; the UI must make that a
      background fact, not a modal wait.
- [ ] 16.8 — OS notifications for new mail and fired reminders, honouring Do Not Disturb, with
      click-to-open. Platform backends behind one trait.
- [ ] 16.9 — Token refresh and re-auth: refresh ahead of expiry on a background task; on a hard
      auth failure, mark the account and prompt once — never a silent stop, and never a prompt loop.

### E17 — Performance and quality

- [ ] 17.1 — `--bench` mode built into the binary, following Ferrite's: generate the fixture,
      re-exec against it so cold-start excludes fixture creation, repeat 5×, report medians, and
      `--bench-compare` as a regression gate in CI.
- [ ] 17.2 — Measure against §4's budgets on macOS and Linux; record results in `BENCH.md` the way
      `spikes/G0-RESULTS.md` does, with machine details.
- [ ] 17.3 — Memory audit: a 200k-message store must not hold 200k parsed messages. Bounded caches
      with explicit eviction for rendered documents, decoded images and formatted strings.
- [ ] 17.4 — Startup: window on screen fast, first frame with real content (Ferrite's bounded
      `apply_loaded_within(60ms)` trick), everything else streaming.
- [ ] 17.5 — Fuzz the untrusted parsers — MIME, HTML, iCalendar, PGP. **Every one of these eats
      bytes a stranger sent you.** `cargo-fuzz` targets in CI.
- [ ] 17.6 — Crash and panic policy: a panic in a sync worker or a renderer must not take the app
      down. Catch at the task boundary, log, surface a degraded state for that message or account.
- [ ] 17.7 — Pixel-diff harness against both handoffs at 1× and 2×, using a throwaway config dir
      and fixed fixture data (Ferrite's `FERRITE_HANDOFF` pattern). **Light is the pixel baseline**
      (it is what the handoffs specify); dark is covered by the 1.3b contrast test plus one manual
      pass per screen, not by its own baseline set.
- [ ] 17.8 — Property tests for the parts where bugs are silent: threading, recurrence expansion,
      the pending-op queue's convergence, and HTML box layout.

### E18 — Packaging and cross-platform hardening

- [ ] 18.1 — macOS `.app` bundle with `Info.plist`, icon, and ad-hoc signing (no notarization
      needed for personal use; document what changes if that stops being true).
- [ ] 18.2 — Windows: a plain build plus whatever minimum makes it launchable from a folder. No
      installer for a personal tool.
- [ ] 18.3 — Linux: build from source, documented in `BUILDING.md`, with the **Vulkan driver** and
      `xdg-desktop-portal` requirements stated up front as hard prerequisites (§1.2).
- [ ] 18.4 — `BUILDING.md` with per-platform system dependencies — cribbed from Zed's own
      `script/linux`, which is the real source of truth (note it needs the `libvulkan1` **loader**,
      not the Vulkan SDK), plus full Xcode on macOS and MSVC-with-Spectre-libs on Windows — and a
      startup capability probe
      (renderer, adapter, software-rendering flag, portal availability, keyring availability) shown
      in the dev overlay and in Settings → Advanced.
- [ ] 18.5 — Full functional pass on Windows and Linux: window chrome, drawn window controls, file
      dialogs, IME, keyboard shortcuts with `Ctrl` instead of `⌘`, notifications, keychain.
- [ ] 18.6 — Fractional-scale pass at 100 / 125 / 150 / 200 %, which is where GPUI's pixel rounding
      shows (§1.2's letter-spacing note came out of exactly this).
- [ ] 18.7 — Data safety: the store survives a kill -9 mid-sync, a full disk, and a schema
      downgrade attempt. A backup/export path — at minimum, "the store is these two directories,
      copy them."

---

## 4. Budgets

Measured on the E0.9 fixture (200k messages, 8 mailboxes, 20k events, 2 accounts), read off the
E0.8 dev overlay, gated in CI by `--bench-compare` (E17.1). The macOS column is the target; the
Linux column is the floor below which the platform is considered broken, not merely slower.

| Metric | macOS (Apple Silicon) | Linux (Vulkan, mid-range) |
|---|---|---|
| Cold start to interactive window | < 400 ms | < 800 ms |
| First frame showing real mail (not an empty list) | < 600 ms | < 1.2 s |
| Message-list scroll, draw p50 / p99 | < 2 / 4 ms | < 8 / 16 ms |
| Click a message → body on screen (cached, plain text) | next frame | next frame |
| Click a message → body on screen (uncached HTML, one screenful) | < 50 ms | < 100 ms |
| Full HTML layout of a long newsletter | < 16 ms per screenful | < 33 ms |
| Search keystroke → first results (200k messages) | < 50 ms | < 120 ms |
| Switch calendar view (month ↔ week) | < 16 ms | < 33 ms |
| Calendar range scroll / month step | next frame | < 33 ms |
| Recurrence expansion, one year of a weekly series | < 1 ms | < 3 ms |
| Incremental sync, both accounts, nothing changed | < 1 s wall, zero visible UI cost | same |
| Idle CPU, window open, synced | < 0.5 % of one core | < 1 % |
| RSS with the 200k fixture loaded | < 250 MB | < 300 MB |
| Store size, 200k messages (DB + cache) | recorded, not capped | — |

The mail list is a strictly easier problem than the seven-column 40k-row track table in the
reference project, which measured **1.76 / 2.86 ms** on the same class of machine. If Snail's list
is slower than that, something is wrong in our code, not in GPUI.

**Two budgets that are about correctness, not speed:**
- **Every view renders with the network unplugged.** Tested by unplugging it.
- **No operation blocks the main thread for more than one frame.** Enforced by a debug assertion on
  main-thread time, not by hoping.

---

## 5. Milestones

| | What it means | Epics |
|---|---|---|
| **M0 — Go/no-go** | The three spikes pass: gpui-kit's text input can carry a compose window, the HTML renderer can read 20 real messages, and both accounts authenticate and pull a page. Nothing is built until they do. | E0 |
| **M1 — It reads mail** | One account (Gmail), window matching the handoff, mail syncs into SQLite, list scrolls, messages render, triage works offline. **The first point where it is worth using.** | E1–E6 |
| **M2 — It replaces your client** | Compose, reply, send, undo send, threading, search, both accounts. Daily-driver on macOS. | E7–E9 |
| **M3 — It reads your week** | Calendar sync from both providers, five views, event editor, recurrence, reminders. | E11–E13 |
| **M4 — It's yours** | PGP, settings, full keyboard, notifications, the states matrix and empty states finished. | E10, E14–E16 |
| **M5 — Three platforms** | Budgets met on macOS and Linux, Windows functional, packaged, documented. | E17, E18 |

**Ordering rationale.** Mail before calendar, because mail is the thing you're in all day and the
calendar is useless without accounts and a store anyway. HTML rendering (E6) sits inside M1 rather
than being deferred, because "reads mail" is not true without it and discovering E6's real cost
late would invalidate every estimate after it. PGP (E10) sits in M4 despite being a v1 requirement,
because it is orthogonal — nothing else depends on it, and it must never be on the path between
you and an ordinary message.

**The two stories most likely to move the schedule** are 6.6 (table layout) and 0.2's fallback
(writing a text input from scratch). Both are known now rather than in month four, which is the
entire point of E0.

---

## 6. Protocol notes

The findings behind E3, E4 and E11, recorded so they are not re-researched. Verified 2026-09-20;
anything marked **[unverified]** needs a live test, and E0.4 is where those happen.

### 6.1 Google: the verification question, and why it doesn't apply

Gmail's mailbox scopes — `gmail.readonly`, `gmail.modify`, `gmail.compose`, `gmail.metadata` and
`https://mail.google.com/` (which covers *all* IMAP/SMTP/POP use) — are **restricted**. Restricted
scopes normally mean Trust & Safety review, a verified domain hosting a public homepage and privacy
policy, a demo video, and an **annual CASA security assessment** by a paid third-party lab
(~$550–$2,000/yr at the lower assurance level, $4,500–$8,000 at the higher; Google charges nothing
itself but does not conduct the assessment either).

**None of that applies here, because of the personal-use exception: fewer than 100 users.** That is
the single most valuable consequence of the "personal tool" decision in the preamble. Worth knowing
precisely what it costs to change that answer later:

- There is **no exemption for open-source, indie, or non-commercial apps.** The 100-user cap is the
  only size-based relief and it applies over the project's entire lifetime — it cannot be reset.
- Switching to the Gmail REST API does **not** avoid restricted scopes. `gmail.modify` is restricted
  too. The only restricted-free configuration is `gmail.send` + `gmail.labels`, i.e. a client that
  cannot read mail.
- CASA is triggered by an app having "the ability to access data from or through a third-party
  server." A genuinely local-only client plausibly falls outside that, which would be a real
  architectural advantage of Snail's no-server design — but Google publishes no explicit
  desktop-client carve-out, so **do not architect on it**. Adding any server component (a push
  relay, a token-exchange proxy, crash reporting that could carry message content) would flip it on.

### 6.2 Google: testing mode vs production

Covered in E0.6, and worth repeating because it is counterintuitive: **testing mode expires refresh
tokens 7 days after consent** — the clock starts at consent, not at last use, and the only scopes
exempted are the sign-in ones (`openid`, `userinfo.*`), which a mail client can never limit itself
to. Production-unverified shows one warning interstitial at setup (Advanced → "Go to … (unsafe)")
and then behaves normally. For a daily driver, production-unverified is the only workable state.

Note that **both** states carry a 100-user cap, but they are different caps: testing limits the
hand-maintained test-user allowlist, while production-unverified caps total users over the
project's entire lifetime, unresettable. At one user, neither matters — but it means "publish to
production" is not a way to grow past 100 later. Google's own policy language for the personal-use
exception is "fewer than 100 people, all of whom are known personally to you."

For the record, if this ever stopped being a personal tool, the realistic path is the one
Thunderbird takes: ship your own client ID and get restricted-scope verified, at roughly
$600–700/year for CASA plus a domain, a privacy policy, and an owner who answers the annual
recertification email. The escape hatch that TUI clients use — make each user create their own
Cloud project — is miserable onboarding for a GUI app and still leaves them facing the same
testing-vs-production choice.

### 6.3 Google: REST vs IMAP for Gmail

Chosen: REST. The reasoning, since the alternative looks superficially simpler:

| | Gmail REST | Gmail IMAP |
|---|---|---|
| Scope | `gmail.modify` (restricted) | `https://mail.google.com/` (restricted) |
| Verification burden | identical | identical |
| Delta sync | `history.list`, 2 units/page — essentially free | CONDSTORE, heavier and less precise |
| Session life | stateless | ~1 hour under OAuth (token lifetime), not 24h |
| Connection limit | none relevant | **15 clients per account**, shared with the user's phone |
| Workspace admin allowlist | not applicable | **can block by OAuth client ID** — auth fails with correct credentials |
| Threading | server-side `threadId`, authoritative | must be computed |

The Workspace allowlist row is the sleeper: on a locked-down domain, IMAP auth fails with a valid
token because the client ID isn't allowlisted, and the error is indistinguishable from bad
credentials unless you handle it specifically.

Note also that Google's own scope documentation contradicts itself — the scope reference says to
request `https://mail.google.com/` "only if your application needs to immediately and permanently
delete threads," while the IMAP page says it is the *only* scope that works for IMAP. Another
reason to avoid that path.

### 6.4 Google: push is unusable, and Google says so

Gmail's `watch` requires Cloud Pub/Sub. Pull subscriptions do remove the public-HTTPS-endpoint
requirement, but not the server: the topic lives in *our* project, so a desktop binary would need an
embedded service-account key; Pub/Sub cannot filter on message *data*, only attributes, and Gmail
puts the user's address inside the data — so one shared subscription would deliver **user A's
mailbox events to user B's machine**. And `watch` must be renewed every ≤7 days including while the
machine is off, which a desktop app structurally cannot do.

Calendar push is worse: HTTPS webhook with a CA-signed certificate, no pull alternative at all.

The Gmail push guide contains an explicit carve-out endorsing polling: for notifications to
user-owned devices such as installed apps, the poll-based sync guide "is still the recommended
approach." **Poll, and cite that if anyone asks.**

### 6.5 Google: the quota model changed on 2026-05-01

The old per-second model is gone. Current: **6,000 units/min/user**, **1.2M units/min/project**, and
a **80,000,000 units/day/project ceiling that cannot be raised**. Per-call costs that matter:
`history.list` 2, `messages.list` 5, `messages.modify` 5, `messages.get` 20, `threads.get` 40,
`messages.send` 100.

Consequences: idle polling is free (a 60-second `history.list` poll is 2,880 units/day), but
**backfill is the constraint** — `messages.get` at 20 units caps you at 300 fetches/min/user, so a
50,000-message mailbox has a **~167-minute wall-clock floor**. That is why E16.7 makes first-run a
background fact rather than a wait. Calendar counts plain requests instead: 600/min/user,
1M/day/project.

**Verified 2026-09-21 (E0.4):** the owner's Gmail is **669,873 messages / 456,442 threads**, so a
full backfill is a **~37-hour** floor at 300/min, not ~3 hours. This makes open question 3
("how much history to backfill") a blocking decision rather than a UX detail, and it means the
E0.9 fixture's 200k messages is *smaller* than the real mailbox.

There is also an **undocumented per-user concurrent-request limit** that returns a 429 and is shared
with every other Gmail client the user is running, including their phone. Hence E16.4's adaptive
limiter.

### 6.6 iCloud: app passwords, and what they don't cover

No OAuth. iCloud IMAP advertises `AUTH=ATOKEN AUTH=PLAIN` only; `ATOKEN` backs Apple's "Account
Data Sharing" delegated flow, which has no public developer documentation, no self-serve
registration and no published protocol — almost certainly partner-only. Sign in with Apple is
identity federation and grants no mail access (someone tested this: iCloud IMAP answers a genuine
Apple OAuth token with `BAD Unsupported authentication`).

So: app-specific passwords, 2FA mandatory, 25 active maximum, **revoked wholesale when the Apple
Account password changes**. Servers `imap.mail.me.com:993` (implicit TLS) and
`smtp.mail.me.com:587` (STARTTLS); no POP; no documented alternate ports.

**No Apple registration of any kind is required** to speak IMAP/SMTP/CalDAV to iCloud. The
`com.apple.developer.icloud-services` entitlement is for CloudKit and is unrelated. The only Apple
gate that touches this project is the $99/yr Developer Program for **notarizing a macOS binary** —
a distribution concern, irrelevant to a personal build, and irrelevant to Linux and Windows.

Apple's iCloud terms prohibit automated access that "interferes with or disrupts" the service and
allow suspension for load caused **unintentionally**. That sentence is the reason E4.21 keeps the
connection pool small and E11.17 backs off hard.

### 6.7 iCloud: the IMAP capability trap

iCloud's **pre-auth** capability string is deliberately thin:
`XAPPLEPUSHSERVICE IMAP4 IMAP4rev1 SASL-IR AUTH=ATOKEN AUTH=PLAIN AUTH=ATOKEN2 AUTH=XOAUTH2`. Only
after login does it reveal
`CONDSTORE ENABLE QRESYNC IDLE UIDPLUS NAMESPACE ESEARCH SORT THREAD QUOTA WITHIN LIST-STATUS ID
XAPPLELITERAL X-APPLE-REMOTE-LINKS …`. Thunderbird polled iCloud instead of using IDLE until
version 80 for exactly this reason.

**Verified 2026-09-21 (E0.4, `spikes/RESULTS.md`):**
- The pre-auth string now also advertises `AUTH=ATOKEN2 AUTH=XOAUTH2` (new since the 2020 dump).
- `ENABLE CONDSTORE` returns OK, **and `SELECT INBOX (CONDSTORE)` also returns OK** on 6739
  messages. The 2020-dump claim that the parameter form yields `BAD Unexpected extra arguments` is
  **out of date**; E4.13(a) can use the simpler `SELECT … (CONDSTORE)`.
- `\NoInferiors` on INBOX, and `[\Sent]`/`[\Trash]` **are** advertised — see E4.18, which said
  iCloud advertises no SPECIAL-USE. `Archive`, `Junk`, `Notes`, `Drafts` carry no flag.

Absent even post-auth: **MOVE, COMPRESS=DEFLATE, LITERAL+, LIST-EXTENDED.** Server lineage looks
like Sun Java System Messaging Server (`X-SUN-IMAP`), not Dovecot, so expect Sun-shaped quirks.

**[unverified]** Whether iCloud files the sent copy server-side (→ whether we APPEND), the SMTP
status code on quota exceedance, UIDVALIDITY stability, and the real connection limit. These need a
send or a longer soak, not a read.

**[unverified]** Whether iCloud files the sent copy server-side (→ whether we APPEND), the SMTP
status code on quota exceedance, UIDVALIDITY stability, and the real connection limit.

### 6.8 iCloud: CalDAV specifics

Sharded onto `pNN-caldav.icloud.com` — follow the redirect, don't pin the shard forever. Principal
paths look like `/8034509913/principal/`.

**Verified 2026-09-21 (E0.4, `spikes/RESULTS.md`):**
- `GET /.well-known/caldav` returns **400**; the well-known is a **DAV resource**, so it must be
  **`PROPFIND`**ed, which returns 207 with `current-user-principal` (`/285963941/principal/`). There
  is no redirect to follow on the well-known itself.
- `calendar-home-set` is a **CalDAV** property (`urn:ietf:params:xml:ns:caldav`). Requesting it in
  the `DAV:` namespace returns a silent 404; once namespaced correctly it resolves to the shard,
  e.g. `https://p146-caldav.icloud.com:443/285963941/calendars/`.
- Multistatus bodies use a **default `DAV:` namespace** (`<response xmlns="DAV:">`), not prefixes.
- The home contains `inbox/`, `outbox/` and `notification/` collections — the RFC 6638 scheduling
  endpoints exist, which E13.8's RSVP probe can use.
- `calendar-color` comes back as `#RRGGBBAA`; strip the alpha.

**[verified 2026-09-21] iCloud supports RFC 6578 `sync-collection`** — `supported-report-set`
advertises it on every calendar, answering the plan's highest-value question in favour of the
incremental path. E11.13 can make `sync-collection` the primary path and keep the ctag+etag diff as
the fallback. (`getctag` did not appear in the Depth-1 PROPFIND and needs a follow-up request.)

Reminders/VTODO are **dead** — Apple disabled Reminders over CalDAV; modern Reminders use a private
protocol. That settles E13.9.

### 6.9 The crate stack

All permissively licensed (MIT / Apache-2.0 / ISC), all checked 2026-09-20.

| Need | Crate | Why, and what to watch |
|---|---|---|
| GPUI | `gpui-kit =0.6.1`, `gpui-pre =0.3.2` | §1.1. Pin exactly. Input/Table/Dock have broken before. |
| IMAP | `async-imap` 0.11.3 (tokio feature) | Delta Chat's engine, so it is battle-tested against real servers. Default feature is async-std — turn it off. **No QRESYNC**; the `imap` crate is on a 19-month-old alpha and asking for maintainers. |
| MIME parse | `mail-parser` 0.11.9 | Zero-copy, actively maintained, and it implements RFC 8621 §4.1.4 body-part selection — the "give me the display body" logic we would otherwise write. Apache-2.0/MIT (the AGPL relicensing people remember applies to the Stalwart *server*, never to these crates). |
| MIME build | `mail-builder` 1.0.0 | Just reached 1.0. |
| SMTP | `mail-send` 0.6.2 | Supports XOAUTH2 **and** OAUTHBEARER; `lettre` has only XOAUTH2 and is still 0.11. |
| iCalendar | `calcard` 0.3.14 | The only crate with real VTIMEZONE resolution *and* a Windows/Exchange TZID→IANA table, plus its own RRULE engine. Pin exactly; 0.3.x moves fast. |
| CalDAV | `libdav` 0.11.0 (ISC) | The only credible client. Pre-1.0 and churns; **implements no incremental sync** — that is ours. |
| Store | `rusqlite` (bundled) + FTS5 | §1.5. |
| HTML | `html5ever` + `ammonia` | §1.4. |
| Time | `chrono` + `chrono-tz` | Not `jiff`, despite jiff being the better calendar-math library — nothing in the iCalendar/CalDAV ecosystem speaks it, so it would mean shims at every boundary. Revisit if `calcard` ever moves. |
| PGP | `pgp` (rpgp) 0.20.0, optionally `rpgpie` | §1.6. Pure Rust, no C dependency, MIT/Apache-2.0, what Delta Chat ships. **Pre-1.0 and breaks API on roughly every minor release** — two breaking releases in 2026 — so pin exactly and budget upgrade time. Does *not* implement certificate semantics (E10.3b) or WKD (E10.7b). |
| WKD | *ours* | No standalone `wkd` crate exists; the alternatives are LGPL, OpenSSL-bound, or pinned to rpgp 0.10 (2024). ~250 lines from the spec. |
| Secrets | `keyring` (apple-native / windows-native / sync-secret-service) | |
| Images | `image` | |
| Paths | `dirs` | |

**Gaps the ecosystem simply does not fill** — say these out loud before estimating:
1. No async IMAP client supports QRESYNC.
2. No IMAP crate ships XOAUTH2 or OAUTHBEARER (they expose a generic `Authenticator` trait). ~20
   lines each, but ours.
3. No CalDAV crate does incremental sync. The engine is ours.
4. Nothing handles OAuth token acquisition/refresh for us.
5. No PGP/MIME layer exists for either OpenPGP library — RFC 3156 canonicalization is ours (E10.5).

---

## 7. Risks

| Risk | Why it's real | Mitigation |
|---|---|---|
| **gpui-kit `Input`/`Textarea` can't carry compose** | 0.6 already broke Input once; Snail leans on it far harder than the reference project (3 inputs) does. The fallback is ~3 weeks. | **E0.2, before anything else.** |
| **E6 (HTML renderer) overruns** | It is the one genuinely hard piece, and table layout (6.6) is a multi-week story inside it. | E0.3 produces the real failure taxonomy up front; 6.13's fallback chain means a message always renders *something*; `ferrite-viz` is a proven template for a webview helper *process* if it has to be abandoned. |
| **GPUI upgrade breaks the app** | Zed doesn't support GPUI as a library; gpui-kit tracks a moving snapshot. | Exact pins everywhere; GPUI usage funnelled through thin helpers (`style.rs`, the tracked-text element, the icon module) so breakage is localized. Never upgrade two things at once. |
| **Google changes the rules** | The quota model changed in May 2026; LSA died; scope classifications have moved. A policy change could require verification or kill the unverified-production path. | The `MailProvider` trait means Gmail-over-IMAP remains a fallback. Keep §6 current. |
| **iCloud is undocumented and unstable** | Half of §6.7–6.8 is inference from 2020 packet dumps and third-party bug trackers. Apple publishes no rate limits and answers no forum threads. | E0.4 captures ground truth; both CalDAV sync paths implemented; conservative connection pool; aggressive backoff. |
| **Recurrence and timezone bugs** | They are silent, and they corrupt the user's actual calendar. | E12.1/12.2/13.4 are pure functions in `snail-ui`/`snail-core` with fixture and property tests, including DST in both directions. `calcard` for VTIMEZONE rather than hand-rolling. |
| **Sync corrupts or loses data** | The worst possible failure for this app. | Local store is a cache, never the only copy of a sent message; `pending_op` is transactional; idempotent sync; kill-tested in CI; full-resync always available from Settings. |
| **Linux has no Vulkan** | GPUI ignores `WGPU_BACKEND` and the GL path panics. Some machines simply cannot run it. | Documented as a hard prerequisite; detected at startup with a real message. Not solvable by us. |
| **PGP certificate semantics** | rpgp explicitly does not implement expiry, revocation or key-flag checking, and will report a signature from a revoked key as cryptographically valid. Getting E10.3b wrong is a real vulnerability, not a cosmetic bug. | Own it as an explicit deliverable with adversarial fixtures (revoked, expired, wrong-flag, SHA-1). `rpgpie` as dependency or reference. Sequoia-with-Nettle is the fallback if the layer outgrows its value. |
| **Scope creep into Thunderbird** | Every mail client does this. Rules, plugins, local folders, Exchange, RSS. | The premise in the preamble is the defence. When in doubt, the answer is no. |
| **Accessibility** | AccessKit has landed in GPUI but is off behind an undocumented env var, the node tree is empty until each component annotates itself, and Windows screen-reader support is openly broken and unassigned upstream. | Prefer gpui-kit widgets, which do publish roles/labels/values, over hand-rolled elements wherever the design allows. Beyond that: acknowledged, tracked, not solved in v1. An honest statement, not a plan. |
| **No software rendering, anywhere** | GPUI has no fallback and Zed actively blocks software Vulkan ICDs. This kills Linux machines without a Vulkan driver *and* Windows over RDP/VDI. | Startup capability probe with a real message (E18.4). Not solvable by us — it is a property of the framework. |

---

## 8. Open questions

1. **What does "kept green" mean for Windows?** E0.10 builds it in CI and E18.5 does a functional
   pass, but nobody is using it daily. Is "compiles, launches, basic flows work" enough, or should
   Windows meet the same budgets as Linux?
2. **Attachment storage policy.** Download everything on sync, or lazily on open? Lazy is lighter
   and faster to first sync; eager makes offline genuinely offline. Affects E2.3 and the backfill
   arithmetic in §6.5.
3. **How much history to backfill.** **Resolved by the owner 2026-09-21: the last 30 days.**
   Older mail is fetched on demand. §6.5's revised arithmetic (669,873 messages → a ~37-hour
   ~full-backfill floor) is why. **Consequence to state in the UI:** local FTS5 search only covers
   the backfilled window, so "search" means "search the last 30 days" until on-demand fetch lands
   in E9 — the empty-result copy must not imply the message does not exist.
4. **The "Other" platform note** from the scoping questions never reached me — if there was a
   specific platform requirement beyond macOS-first with Linux and Windows green, it is not in
   this plan.

---

## 9. What exists on disk today

| Path | What it is |
|---|---|
| `design_handoff_email_client/` | README (123 lines) + `Mail Mockups.dc.html` (4 screens: inbox, thread, compose, settings) + the `dc-runtime` support bundle. |
| `design_handoff_almanac_calendar/` | README (501 lines — the more detailed of the two, with a full state model and suggested Rust entities) + `Almanac Calendar.dc.html` (7 screens) + `Titlebar.dc.html` + `Sidebar.dc.html`. |
| — | **Note:** the calendar mockups reference `./support.js`, which is not in that directory. Copy it from the mail handoff to view them. |
| `plan.md` | This document. |

Nothing else. No Cargo workspace yet; E0.1 creates it.

## 10. Reference material

- `~/projects/foobar2001/GPUI.md` — the GPUI port plan that settled the dependency question,
  the per-platform matrix, and the risk table this plan's §7 is modelled on.
- `~/projects/foobar2001/spikes/G0-RESULTS.md` — measured go/no-go results on macOS and a Pi 5.
  The source of every performance number and most of the gotchas in §1.2.
- `~/projects/foobar2001/crates/ferrite-gpui/` — 21.7k lines of working GPUI: `main.rs` for
  bootstrap, `actions.rs` for the command-table pattern, `style.rs` for the no-raw-colours rule,
  `tracked.rs` for letter-spacing, `bench/` for the benchmark harness.
- `~/projects/foobar2001/crates/ferrite-ui/` — the framework-free UI crate `snail-ui` copies.
- `~/projects/foobar2001/crates/ferrite-services/src/hub.rs` — the `Hub<T>` → channel → `cx.spawn`
  bridge that E16 uses for sync events.
