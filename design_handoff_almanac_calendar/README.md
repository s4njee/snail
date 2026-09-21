# Handoff: Almanac — desktop calendar (Rust / Tauri)

## Overview
Almanac is a full-featured desktop calendar for personal and school life: five calendar
views, multiple calendars with colors, recurring events, reminders, tasks, and CalDAV /
Google sync. This bundle documents seven screens covering the whole surface of the app.

Target shell: **Tauri 2.x** with a Rust backend and a local SQLite store. The window is
**frameless** — the app draws its own titlebar, including window controls.

## About the Design Files
The files in this bundle are **design references created in HTML**. They are prototypes
that show intended look, layout and content — **not production code to copy directly**.

The task is to **recreate these designs in the target codebase's own environment**, using
its established patterns, component library and styling approach. If the Tauri project has
no frontend yet, pick the framework that suits the team (React, Svelte, Solid, Vue, or
plain TS) and implement the designs there. Nothing in these files is required at runtime.

The HTML uses a streaming component format with inline styles only. Read it as a
specification of geometry and color, not as an architecture to mirror.

## Fidelity
**High-fidelity.** Colors, typography, spacing and copy are final. Recreate the UI closely
using the codebase's own libraries. Interaction states beyond the ones listed below
(drag-to-create, keyboard navigation, virtualized scrolling) are **not** designed and are
left to implementation judgement.

All screens are drawn at a fixed **1440 × 900** window. Responsive rules are not designed;
see *Responsive behavior* for the intended stretch/collapse order.

---

## Design Tokens

### Neutrals (grey, no warm tint)
| Token | Hex | Use |
| --- | --- | --- |
| `desk` | #E4E4E4 | Canvas behind the window (presentation only) |
| `chrome` | #FAFAFA | Titlebar background, window base |
| `sidebar` | #F4F4F4 | Left sidebar, settings nav |
| `surface` | #FFFFFF | Main content panes, cards, popovers |
| `surface-2` | #FBFBFB | Right rail, card headers, footer bars |
| `surface-3` | #FCFCFC | Month weekday header strip, all-day band |
| `segment-track` | #EDEDED / #E4E4E4 | Segmented-control track (settings / titlebar) |
| `border` | #E0E0E0 | Input borders, sidebar divider, button outlines |
| `border-soft` | #E5E5E5 | Header rules, card outlines |
| `grid` | #EBEBEB | Calendar cell borders, column separators |
| `grid-hour` | #F2F2F2 | Hour lines inside time grids |
| `ink` | #1A1A1A | Primary text |
| `ink-event` | #232323 / #262626 | Event block titles / month chip titles |
| `ink-2` | #424242 | Mini-month date numbers |
| `ink-3` | #575757 | Body copy in modal |
| `ink-4` | #666666 | Secondary body copy, place names |
| `ink-5` | #777777 | Inactive segment labels, arrows |
| `ink-6` | #888888 | Meta text, window controls |
| `ink-7` | #A0A0A0 | Section labels, muted mono |
| `ink-8` | #B3B3B3 | Hour labels |
| `ink-9` | #BFBFBF | Disabled / out-of-month dates |
| `dashed` | #CACACA | Dashed "add" affordances |
| `track-off` | #D9D9D9 | Toggle track, off |

### Accent
| Token | Hex | Use |
| --- | --- | --- |
| `accent` | #1F6FEB | Today, selection, primary button, now-line, active repeat days |
| `accent-tint` | #E4EBF8 | Reminder chips, selected mini-month day |
| `today-wash` | #FBFCFE | Today's month cell / week column header |
| `today-wash-2` | #FDFDFE | Today's week column body |

### Calendar colors
Each calendar has a solid color, a 10% tint for event fills, and a name.

| Calendar | Color | Tint |
| --- | --- | --- |
| Classes | #C2410C | rgba(194,65,12,0.10) |
| Personal | #1F6FEB | rgba(31,111,235,0.10) |
| Work shifts | #0F766E | rgba(15,118,110,0.10) |
| Birthdays | #7C5CBF | rgba(124,92,191,0.10) |
| Home | #A16207 | rgba(161,98,7,0.10) |
| US Holidays (off) | #888888 | — |

A disabled calendar renders its swatch as #BFBFBF and its label as #A0A0A0.

### Typography
Two families, both Google Fonts:

```
Bricolage Grotesque — opsz 12..96, wght 300..700   (all UI text, display numerals)
DM Mono            — wght 300;400;500              (times, dates, counts, labels, shortcuts)
```

| Role | Family | Size | Weight | Tracking |
| --- | --- | --- | --- | --- |
| Month title ("August") | Bricolage | 42px | 500 | -0.03em |
| Week / view title | Bricolage | 34px | 500 | -0.03em |
| Settings pane title | Bricolage | 32px | 500 | -0.03em |
| Day-view numeral ("13") | Bricolage | 76px | 400 | -0.045em |
| Agenda date numeral | Bricolage | 46px | 400 | -0.045em, line-height 0.82 |
| Modal title | Bricolage | 26px | 500 | -0.025em |
| Day-view weekday | Bricolage | 22px | 500 | -0.02em |
| Week column date | Bricolage | 19px | 500 | -0.02em |
| 3-day column date | Bricolage | 24px | 500 | -0.02em |
| Account name | Bricolage | 14.5px | 500 | -0.01em |
| Agenda row title | Bricolage | 14px | 400 | — |
| Body / list item | Bricolage | 12.5–13px | 400–500 | — |
| Event block title (week) | Bricolage | 11.5px | 500 | line-height 1.25 |
| Event block title (3-day) | Bricolage | 13px | 500 | — |
| Event block title (day) | Bricolage | 15px | 500 | -0.01em |
| Month chip title | Bricolage | 11px | 400 | — |
| Year suffix ("2026") | DM Mono | 20px / 17px / 15px | 400 | — |
| Time on event block | DM Mono | 9.5px (week) / 10px (3-day) / 11px (day) | 400 | — |
| Hour gutter | DM Mono | 10px / 10.5px / 11px | 400 | — |
| Section label (CALENDARS, TASKS…) | DM Mono | 9.5px | 400 | 0.12em, uppercase |
| Field label (WHEN, REPEATS…) | DM Mono | 10px | 400 | 0.08em, uppercase |
| Weekday header (SUN…) | DM Mono | 9.5px | 400 | 0.12em |
| Calendar-name tag in agenda | DM Mono | 9.5px | 400 | 0.06em, uppercase |

### Radius
2px (bar caps) · 3px (small swatch) · 3.5px (mini-month swatch) · 5px (month chip) ·
6px (week event, mini-month cell, date pill, segment item) · 7px (nav row, agenda row,
repeat-day) · 8px (button, input, pill) · 9px (day event, segment track, account avatar) ·
10–15px (toggle / chip, fully round) · 11px (account card) · 12px (window) · 14px (modal)

### Shadow
| Name | Value |
| --- | --- |
| window | `0 24px 60px -18px rgba(0,0,0,0.34), 0 0 0 1px rgba(0,0,0,0.08)` |
| modal | `0 40px 80px -20px rgba(0,0,0,0.5)` |
| segment active | `0 1px 2px rgba(0,0,0,0.10)` |
| event block (week/3-day) | `0 1px 2px rgba(0,0,0,0.05)` |
| event block (day) | `0 1px 3px rgba(0,0,0,0.06)` |

### Spacing
4px base. Common: 2, 3, 5, 7, 9, 10, 12, 14, 16, 18, 20, 26, 34, 40px.
Pane padding: sidebar 18px, main header 26px, agenda 34px, settings 40px.

### Modal scrim
`rgba(0,0,0,0.34)`, covering everything **below the titlebar** (inset 52px 0 0 0) so the
window chrome stays live.

---

## Shared Chrome

### Titlebar — 52px, full width
Background #FAFAFA, 1px bottom border #E0E0E0. Whole bar is the OS drag region except the
interactive controls. Left→right, 14px gaps, 0 14px padding:

1. **Wordmark** — 16px #1A1A1A rounded square (radius 4px) + "Almanac", 14px/600, -0.01em.
2. **View switcher** — segmented control. Track #E4E4E4, radius 9px, 3px padding, 2px gaps.
   Items: Month · Week · 3-day · Day · Agenda. Item padding 5px 11px, radius 6px, 12.5px/500.
   Active: #FFFFFF fill, #1A1A1A text, segment-active shadow. Inactive: transparent, #777777.
3. **Spacer** (flex 1).
4. **Search** — 230 × 30px, #FFFFFF, 1px #E0E0E0, radius 8px, 0 10px. Contains an 11px
   circle outline (1.5px #A0A0A0), placeholder "Search or type a date" in DM Mono 11.5px
   #A0A0A0, and "⌘K" in DM Mono 10.5px #BFBFBF pinned right.
5. **Primary button** — "+ New event", 30px tall, 0 12px, radius 8px, #1F6FEB / #FFFFFF,
   12.5px/500, 7px gap, leading "+" at 14px.
6. **Divider** — 1 × 20px #E0E0E0, 2px side margins.
7. **Window controls** — 14px gaps, #888888: minimize = 11 × 1.5px bar; maximize = 10px
   square, 1.5px border, radius 2px; close = two 12 × 1.5px bars rotated ±45°.

Active view per screen: Month → Month, Week → Week, 3-day → 3-day, Day → Day,
Agenda → Agenda, Event editor → Week, Settings → Month.

### Sidebar — 264px, full height
#F4F4F4, 1px right border #E0E0E0. Column of four blocks separated by 1px #E0E0E0 rules
inset 18px:

1. **Mini month** (padding 18px 18px 14px)
   - Header row: "August 2026" 14px/600, -0.01em; "‹ ›" in DM Mono 12px #A0A0A0, 12px gap.
   - Weekday letters S M T W T F S — DM Mono 9.5px #A0A0A0, 0.06em, centered, 7-col grid.
   - 6 × 7 grid, 2px gaps, cells 27px tall, radius 6px, DM Mono 11.5px.
     Out-of-month #BFBFBF · in-month #424242 · selected #E4EBF8 bg, weight 500 ·
     today #1F6FEB bg, #FFFFFF text, weight 500.
   - Grid runs Sun 26 Jul → Sat 5 Sep (42 cells). Today = 13.
2. **CALENDARS** (padding 16px 18px, 9px row gap)
   Row = 12px swatch (radius 3.5px, 1.5px border in the calendar color, filled when
   enabled) + name 12.5px + right-aligned event count in DM Mono 10.5px #BFBFBF.
   Rows: Classes 18 · Personal 24 · Work shifts 9 · Birthdays 4 · Home 6 ·
   US Holidays 2 (disabled).
3. **TASKS** (padding 16px 18px, 11px row gap; header has a trailing "+" in DM Mono 11px)
   Row = 13px circle (1.5px border) + title 12.5px + due line DM Mono 10px.
   - open: ring #B3B3B3, title #1A1A1A, meta #A0A0A0
   - overdue: ring #C2410C, meta #C2410C
   - done: ring and fill #BFBFBF, title #A0A0A0 with line-through
   Rows: "Lab report — draft 2 / Due today · 17:00" (overdue) · "Read Ch. 4–6, Stats /
   Tomorrow" · "Renew bus pass / Sat 15 Aug" · "Email advisor / Done 11:20" (done).
4. **Sync footer** (13px 18px, 1px top border) — 7px #0F766E dot, "3 accounts synced"
   11.5px/500 over "last 2 min ago" DM Mono 9.5px #A0A0A0, and a vertical 3-dot menu
   (3 × 3px #A0A0A0 dots, 2.5px gaps) on the right.

---

## Screens

### 01 — Month
**Purpose:** default view; scan the month, spot overflow days, jump to a date.

**Layout:** titlebar 52 → row [sidebar 264 | main 1176]. Main is a column:
header 88px → weekday strip 30px → 6 × 7 grid filling the rest (~122px rows).

- **Header** (0 26px, 1px bottom #E5E5E5, 18px gaps): "August" 42px/500 baseline-aligned
  with "2026" DM Mono 20px #A0A0A0 (10px gap) · 1 × 26px divider · "week 33 · 4 calendars
  shown" DM Mono 11.5px #888888 · spacer · date stepper.
- **Date stepper:** 30px tall, 1px #E0E0E0, radius 8px, three cells split by 1px rules —
  "‹" 32px wide, "Today" 12.5px/500 with 13px side padding, "›" 32px wide.
- **Weekday strip:** #FCFCFC, 1px bottom #E5E5E5, 7 columns, labels left-padded 9px.
- **Cell:** 1px right+bottom #EBEBEB, padding 7px 7px 0, 3px gaps, overflow hidden.
  Background #FFFFFF; out-of-month #FCFBF8 → use `surface` with #BFBFBF numerals;
  today #FBFCFE.
  - Date pill: min 21 × 21px, radius 6px, DM Mono 12px. Today = #1F6FEB / #FFFFFF.
  - Optional day tag beside it in DM Mono 9px #B3B3B3, 0.06em: TODAY (13), MOVE-IN (17),
    TERM (24).
  - Up to **3 event chips**, then "+N more" in DM Mono 9.5px #A0A0A0 padded 6px left.
  - Chip: 3px 6px, radius 5px, calendar tint background, 2.5 × 11px color bar (radius 2px),
    time in DM Mono 9px in the calendar color ("•" when all-day), title 11px #262626,
    single line, ellipsis.

**Content:** 3 Tuition due · 4 9:00 Dentist · 5 18:00 Study group · 6 Farmers market ·
9 Long run + Meal prep · 10 Gym, Stats 101, Registration opens (+2 more) · 11 Intro Bio,
Advisor meeting, Problem set (+1 more) · 12 Gym, Stats 101, Bio lab · 13 Stats 101 lecture,
Career fair, Dinner w/ Priya (+2 more) · 14 Intro Bio, Work shift · 15 Hiking trip, Mom's
birthday · 17 Move-in day · 18 Orientation · 19 Textbook pickup · 20 Intro Bio, Work shift ·
21 Payday · 22 Concert — The Bell · 24 Classes begin, Stats 101, Campus tour (+1 more) ·
25 Work shift · 26 Stats 101, Study group · 27 Flu shot, Bio lab · 28 Quiz 1, Work shift ·
29 Beach day · 31 Rent due.

### 02 — Week
**Purpose:** the working view — see the shape of the week and the current moment.

**Layout:** header 78 → column header 52 → all-day band 40 → time grid (fills).

- **Header:** "9 – 15 August" 34px/500 + "2026" DM Mono 17px · spacer · "31h scheduled"
  DM Mono 11.5px #888888 · date stepper.
- **Column header:** 62px empty gutter, then 7 equal columns (1px right #EBEBEB), each
  centered: weekday DM Mono 9.5px 0.1em over date 19px/500. Today (Thu 13) uses #1F6FEB
  for both and a #FBFCFE wash.
- **All-day band:** #FCFCFC, gutter label "ALL-DAY" DM Mono 9px #B3B3B3 right-aligned.
  Chip = 4px 7px, radius 5px, tint fill, 2.5 × 11px bar, title 11px.
  Wed 12 "Tuition due" (Classes) · Thu 13 "Payday" (Work) · Sat 15 "Mom's birthday".
- **Time grid:** 07:00 → 21:00, **15 rows at 45px pitch** (44px + 1px #F2F2F2 line).
  Gutter labels DM Mono 10px #B3B3B3, nudged -5px so they sit on the line.
  Today's column body gets a #FDFDFE wash.
- **Event block:** absolutely positioned, `left:2px; right:5px`,
  `top = (start − 7) × 45`, `height = duration × 45 − 3`. Padding 5px 7px, radius 6px,
  tint background, **2.5px left border** in the calendar color, event-block shadow.
  Inside: title 11.5px/500 #232323 (ellipsis) over "HH:MM – HH:MM" DM Mono 9.5px in the
  calendar color.
- **Now-line:** in today's column only — full-width 1.5px #1F6FEB at `top:300px` (13:40),
  with an 8px #1F6FEB dot at `left:-4px`.

**Content (start, hours, title, calendar, place):**
- Sun 9 — 9:30 1h Long run (Personal, Burke-Gilman) · 15:00 1.5h Meal prep (Home, Kitchen)
- Mon 10 — 8:00 1h Gym — legs (Personal, IMA) · 10:00 1.5h Stats 101 (Classes, Kane 210) ·
  13:00 0.5h Registration opens (Classes, Online) · 16:30 4h Work shift (Work, Campus store)
- Tue 11 — 9:00 1.5h Intro Bio (Classes, Hitchcock 132) · 11:00 0.5h Advisor meeting
  (Classes, Mary Gates) · 14:00 2h Library — problem set (Classes, Suzzallo) ·
  19:00 1h Roommate call (Personal, Home)
- Wed 12 — 8:00 1h Gym — push (Personal, IMA) · 10:00 1.5h Stats 101 (Classes, Kane 210) ·
  12:30 1h Lunch w/ Dana (Personal, The Ave) · 15:00 3h Bio lab (Classes, Hitchcock 4)
- Thu 13 — 10:00 1.5h Stats 101 lecture (Classes, Kane 210) · 12:00 2h Career fair
  (Classes, HUB Ballroom) · 15:00 2h Lab report — writing (Classes, Suzzallo) ·
  19:30 2h Dinner w/ Priya (Personal, Kedai Makan)
- Fri 14 — 8:00 1h Gym — pull (Personal, IMA) · 9:00 1.5h Intro Bio (Classes,
  Hitchcock 132) · 11:00 1h Quiz review (Classes, Kane 210) · 17:00 5h Work shift
  (Work, Campus store)
- Sat 15 — 7:30 7h Hiking trip — Cascade Pass (Personal, Trailhead 7:30)

### 03 — 3-day
**Purpose:** the near horizon at a density where detail fits on the block.

Same structure as Week with three columns (Thu 13 – Sat 15). Header title
"Thu 13 – Sat 15" + "Aug 2026" DM Mono 17px; meta "next 3 days".

- **Column header 56px:** left-aligned, 16px padding, 10px gap — date 24px/500, then a
  stack of weekday 12px/500 over a note in DM Mono 9.5px #A0A0A0:
  "4 events · 1 task due" · "4 events" · "1 event · all day".
- **Gutter 70px**, labels DM Mono 10.5px, nudged -6px. **15 rows at 47px pitch**
  (46px + 1px line).
- **Event block:** same geometry math at 47px pitch, padding 9px 12px. Three lines:
  title 13px/500, time DM Mono 10px, place 11px #666666.
- **Now-line** at `top:313px` with an 8px dot and a "13:40" flag — DM Mono 9.5px #1F6FEB
  on #FFFFFF, 1px 4px, radius 3px, pinned 6px from the right, -8px vertical.

### 04 — Day
**Purpose:** one day in full, with everything due, reminded or repeating beside it.

**Layout:** titlebar → [sidebar 264 | day pane (flex) | right rail 300].

- **Header 106px:** "13" at 76px/400, -0.045em, next to a stack of "Thursday" 22px/500 and
  "August 2026 · week 33" DM Mono 12px #A0A0A0; date stepper on the right.
- **Grid:** gutter 76px (DM Mono 11px, -6px nudge), **15 rows at 49px pitch** (48 + 1px).
- **Event block:** `left:8px; right:20px`, `top = (start − 7) × 49`,
  `height = duration × 49 − 4`, padding 12px 16px, radius 9px, tint fill, **3px left
  border**, day-event shadow. Left stack: title 15px/500, time DM Mono 11px, place 12px
  #666666. Right, flush top: a meta tag in DM Mono 9.5px 0.06em in the calendar color —
  "MON WED THU · REPEATS" (Stats 101), "FOCUS BLOCK" (Lab report), "REMINDER 18:30"
  (Dinner). Career fair has none.
- **Now-line** at `top:301px`, full width, 1.5px #1F6FEB + 8px dot.
- **Right rail 300px:** #FBFBFB, 1px left #E5E5E5, sections split by 1px rules inset 20px.
  - **DUE TODAY** — two cards: #FFFFFF, 1px #E5E5E5, radius 8px, 10px 11px, 13px circle
    checkbox + title 12.5px/500 + meta DM Mono 10px.
    "Lab report — draft 2 / 17:00 · Intro Bio" (ring and meta #C2410C) ·
    "Print career-fair résumés / before 12:00" (ring #B3B3B3, meta #A0A0A0).
  - **REMINDERS** — rows of a 52px DM Mono 10.5px #777777 time column + 12.5px label:
    09:50 Stats 101 in 10 min · 18:30 Leave for dinner · 21:00 Pack for hiking trip.
  - **REPEATS** — "Stats 101 lecture / Mon Wed Thu · until 18 Dec" ·
    "Gym block / every weekday · 08:00" (meta DM Mono 10px #A0A0A0).
  - **Footer** (1px top border, 16px 20px): 32px "Add to this day" button, 1px dashed
    #CACACA, radius 8px, 12px #777777.

### 05 — Agenda
**Purpose:** a continuous list forward from today; dates set as graphics down the left.

**Layout:** header 78 (padding 0 34px) → list (padding 0 34px).

- **Header:** "Agenda" 34px/500 + "from today" DM Mono 15px #A0A0A0 · spacer · a segmented
  control on an #EDEDED track: "Events" (active, #FFFFFF + shadow) / "Events + tasks".
- **Day group:** 22px vertical padding, 1px bottom #EBEBEB, 28px gap between the date
  block and the rows.
  - **Date block 132px:** numeral 46px/400, -0.045em, line-height 0.82, beside a stack
    (3px top padding) of weekday 12.5px/500 and month DM Mono 9.5px 0.06em #A0A0A0.
    Today's numeral and weekday are #1F6FEB; month reads "AUG · TODAY".
  - **Row:** 8px 10px, radius 7px, zebra — odd rows #FBFBFB. Columns: time DM Mono 11.5px
    #777777 at fixed 104px · 3 × 16px calendar bar (radius 2px) · title 14px, ellipsis ·
    spacer · place 11.5px #888888 · calendar name DM Mono 9.5px 0.06em in the calendar
    color, 74px right-aligned.

**Content:** Thu 13 (Stats 101 lecture / Career fair / Lab report — writing / Dinner w/
Priya) · Fri 14 (Gym — pull / Intro Bio / Work shift) · Sat 15 (Mom's birthday all day /
Hiking trip — Cascade Pass) · Mon 17 (Move-in day all day, Alder Hall / Key pickup,
Housing office) · Tue 18 (Orientation, Meany Hall / Work shift).

### 06 — Event editor
**Purpose:** create or edit an event, including recurrence and reminders, in one sheet.

Rendered as a modal over a dimmed Week view. Scrim `rgba(0,0,0,0.34)` inset 52px from the
top. Sheet: 576px wide, `top:96px`, horizontally centered, #FFFFFF, radius 14px,
modal shadow.

- **Head** (22px 26px 18px, 1px bottom #EBEBEB): "EDIT EVENT" DM Mono 9.5px 0.12em
  #A0A0A0, then the title as an inline field — 26px/500, -0.025em, 8px bottom padding,
  **1.5px bottom border #1F6FEB** (focused state). Value "Stats 101 lecture".
- **Body** (20px 26px, 18px row gap). Every row is a 92px DM Mono 10px 0.08em #A0A0A0
  label + control, 16px gap. Inputs: 34px tall, 1px #E0E0E0, radius 8px, 0 12px, 13px text;
  date/time inputs use DM Mono 12.5px. Dropdowns end in a "▾" DM Mono 10px #A0A0A0.
  - **CALENDAR** — 11px #C2410C swatch (radius 3px) + "Classes".
  - **WHEN** — "Thu 13 Aug 2026" · "10:00" · "→" #A0A0A0 · "11:30" · spacer ·
    "All day" 12px #777777 + toggle (32 × 19px, radius 10px, track #E0E0E0, 15px knob,
    off).
  - **REPEATS** — stacked: "Weekly" dropdown · a row of 7 day buttons (34 × 30px,
    radius 7px, DM Mono 11px; on = #1F6FEB fill, #FFFFFF text, matching border; off =
    #FFFFFF, #888888, 1px #E0E0E0) with **M, W, T(hu) on** · "Ends" 12.5px #575757 +
    "on 18 Dec 2026" (30px, radius 7px) + "54 occurrences" DM Mono 10.5px #A0A0A0.
  - **REMIND ME** — chips 30px tall, radius 15px, #E4EBF8, DM Mono 11px #1F6FEB with a
    "×": "10 min before", "at 08:00 same day"; then a 30px circular dashed "+".
  - **WHERE** — "Kane Hall 210".
  - **NOTES** — min-height 64px textarea, 13px/1.5 #575757: "Bring the problem set.
    Office hours right after in Padelford C-8."
- **Foot** (16px 26px, 1px top #EBEBEB, #FBFBFB): "Delete" 12.5px #C2410C · spacer ·
  "Cancel" (34px, 1px #E0E0E0, radius 8px, #FFFFFF) · "Save event" (#1F6FEB / #FFFFFF,
  34px, 0 17px, radius 8px, 13px/500, with "⌘↵" DM Mono 10.5px at 75% opacity).

### 07 — Settings / Accounts
**Purpose:** connect CalDAV, Google and local stores; toggle individual calendars.

**Layout:** titlebar → [settings nav 236 | pane (flex)]. The main sidebar is replaced.

- **Nav:** #F4F4F4, 1px right #E0E0E0, padding 20px 14px, 3px row gaps. "SETTINGS" label
  DM Mono 9.5px 0.12em #A0A0A0 (0 10px 12px). Rows 34px, 0 10px, radius 7px, 13px:
  General · **Accounts** (active: #FFFFFF fill, #1A1A1A, weight 500) · Calendars ·
  Notifications · Appearance · Keyboard · Advanced (inactive #666666). Footer, 0 10px:
  "Almanac 1.4.2" DM Mono 10px #A0A0A0 over "tauri 2.4 · sqlite" DM Mono 10px #BFBFBF.
- **Pane head** (30px 40px 22px, 1px bottom #E5E5E5): "Accounts" 32px/500 over
  "Events sync in the background and stay readable offline in the local store." 14px
  #666666, 6px above.
- **Body** (26px 40px, 16px gaps): three account cards, then an add row, then a sync row.
- **Account card:** 1px #E5E5E5, radius 11px, overflow hidden.
  - Header (16px 18px, #FBFBFB, 14px gaps): 34px avatar, radius 9px, calendar-tint fill,
    initials in DM Mono 13px in the accent color · name 14.5px/500 over detail DM Mono
    10.5px #888888 · spacer · 7px status dot + status DM Mono 10.5px · "Sync now"
    (30px, 1px #E0E0E0, radius 8px, #FFFFFF, 12px).
  - Body (14px 18px, wrapping 10px gaps): per-calendar pills — 30px tall, 0 12px,
    radius 8px, 1px border; 10px swatch (radius 3px) + name 12.5px + a 28 × 17px toggle
    (radius 9px, 13px knob at 2px/13px). On: #FFFFFF fill, 1px #E0E0E0, #1F6FEB track.
    Off: #FAFAFA fill, 1px #EBEBEB, #A0A0A0 label, #BFBFBF swatch, #D9D9D9 track.
  - Cards: **iCloud — CalDAV** / caldav.icloud.com · casey@icloud.com / #0F766E dot,
    "synced 2 min ago" / Personal, Home, Birthdays on, Archive 2024 off.
    **Google** / casey.pham@gmail.com / #A16207 dot, "syncing…" / Classes, Work shifts on,
    US Holidays off. **On this Mac** / local store · no network / #888888 dot,
    "offline only" / Scratch, Tasks on.
- **Add row:** 46px, 1px dashed #CACACA, radius 11px, centered 13px #777777 —
  "+ Add CalDAV, Google or ICS subscription".
- **Sync row:** "SYNC EVERY" DM Mono 10px 0.08em #A0A0A0 · segmented control on #EDEDED
  (DM Mono 11.5px): 5 min · **15 min** (active) · 1 hour · manual · spacer ·
  "local store · 2.1 MB · 1,284 events" DM Mono 10.5px #A0A0A0.

---

## Interactions & Behavior
Designed and specified above:
- View switcher, date stepper (‹ / Today / ›), and the sidebar's mini-month arrows change
  the visible range. The mini-month selection follows the focused date.
- Calendar rows in the sidebar and pills in Settings toggle visibility; a disabled calendar
  drops out of every view immediately and desaturates its swatch.
- "+ New event" and "Add to this day" open the event editor sheet. Clicking an existing
  event opens the same sheet populated. ⌘↵ saves, Esc cancels, ⌘K focuses search.
- Reminder chips are removable via their "×"; the dashed "+" adds another.
- Repeat day buttons are a multi-select; changing them updates the occurrence count.
- "Sync now" per account; the global cadence control sets the background interval.

Not designed — implement to taste, matching the visual language:
- Hover and focus states. Suggestion: event blocks lift to the day-event shadow; rows and
  buttons darken one neutral step; focus rings use #1F6FEB at 2px.
- Drag to create, drag to move, edge-resize on the time grids.
- Overlapping events in a column (side-by-side splitting) — none of the sample data
  overlaps, so no rule is drawn.
- Scrolling: the time grids are cropped to 07:00–21:00 and the agenda list is cropped at
  the window edge. Both should scroll; the grid should open scrolled to the current hour.
- Empty states, first-run, sync failure, and permission prompts.
- Animations. Keep them short — 120–180ms ease-out for view changes and sheet entry.

## Responsive behavior
Designed at 1440 × 900 only. Intended order as the window narrows: the day-view right rail
collapses first (below ~1200px), then the sidebar becomes a toggled overlay (below
~1000px), then the view switcher collapses to a dropdown. Time-grid columns and month cells
stretch to fill; the hour pitch stays fixed.

## State Management
Frontend state:
- `view` — month | week | threeDay | day | agenda
- `focusedDate` — the anchor date; derives every visible range
- `selectedDate` — mini-month highlight
- `visibleCalendarIds` — set of enabled calendars
- `editor` — { open, mode: create | edit, draft: EventDraft, dirty }
- `agendaMode` — events | eventsAndTasks
- `settings` — { pane, syncIntervalMinutes }
- `now` — ticking clock for the now-line; refresh on a minute boundary
- `syncStatus` — per account: idle | syncing | error, plus `lastSyncedAt`

Data loading: fetch the expanded occurrences for the visible range plus a one-range buffer
on either side. Recurrence expansion belongs in Rust, not the frontend. Reminders fire from
the backend so they work when the window is closed.

## Suggested Rust / Tauri surface
Not part of the visual design — a starting point that matches what the screens need.

```rust
// Entities
Account   { id, kind: Caldav | Google | Local, display_name, detail, last_synced_at, status }
Calendar  { id, account_id, name, color, enabled, event_count }
Event     { id, calendar_id, uid, title, location, notes, starts_at, ends_at,
            all_day, tz, rrule: Option<String>, exdates, updated_at, etag }
Occurrence{ event_id, starts_at, ends_at, all_day }   // expanded, not stored
Reminder  { id, event_id, offset_minutes: Option<i64>, absolute_at: Option<DateTime> }
Task      { id, title, due_at, completed_at, calendar_id }
```

```rust
// Commands
list_occurrences(from, to, calendar_ids) -> Vec<Occurrence + Event summary>
get_event(id) -> Event
save_event(EventDraft) -> Event          // create or update; scope: this | future | all
delete_event(id, scope)
set_calendar_enabled(calendar_id, enabled)
list_accounts() -> Vec<Account + Vec<Calendar>>
add_account(AccountSpec) -> Account      // CalDAV URL / OAuth / ICS URL
sync_account(account_id) -> SyncReport
set_sync_interval(minutes)
list_tasks(from, to) -> Vec<Task>
toggle_task(id)
search(query) -> Vec<Occurrence>         // also parses natural-language dates for ⌘K
```

Events to emit: `sync:started`, `sync:finished`, `sync:error`, `data:changed`
(so open views refetch), `reminder:fired`.

Frameless window: set `decorations: false` in `tauri.conf.json`, mark the titlebar as the
drag region, and wire the three window controls to minimize / toggle-maximize / close.

## Assets
None. No images, no icon font, no SVG artwork. Every glyph in the design is either text
(‹ › → ▾ × + ⌘K ⌘↵) or a plain div — bars, squares, circles, dashed outlines. Substitute
the codebase's own icon set where an icon is clearer than the primitive.

Fonts load from Google Fonts:
`https://fonts.googleapis.com/css2?family=Bricolage+Grotesque:opsz,wght@12..96,300..700&family=DM+Mono:wght@300;400;500&display=swap`
For a packaged desktop app, self-host both families instead of fetching at runtime.

## Files
| File | Contents |
| --- | --- |
| `Almanac Calendar.dc.html` | All seven screens, stacked top to bottom. Layout in the template; sample data and computed geometry in the logic class at the bottom of the file. |
| `Titlebar.dc.html` | The 52px frameless titlebar. Takes an `active` view name. |
| `Sidebar.dc.html` | The 264px sidebar. Takes a `sel` date string for the mini-month. |

Sample data lives in the logic class of `Almanac Calendar.dc.html`: `CAL` (calendar
colors), `MONTH` (month chips), `WEEK` (week and 3-day events), `ALLDAY`, `AG`
(agenda groups), and the `accounts` array. Read those for exact copy and times.
