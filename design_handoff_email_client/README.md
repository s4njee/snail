# Handoff: Lightweight Email Client (macOS desktop)

## Overview
Four hi-fi screens for a deliberately small desktop mail client: a three-pane inbox, a thread reading view, a compose window, and a settings window. The product premise is "fewer features": no rules, no smart folders, no VIPs, no flags. Mailboxes are the five standard ones plus per-account entries, and settings fits on a single scrollless panel.

## About the Design Files
The file in this bundle is a **design reference created in HTML** — a prototype showing intended look and layout, not production code to copy. Recreate these screens in the target codebase's existing environment (React, SwiftUI, Electron, etc.) using its established component patterns and libraries. If no environment exists yet, pick the framework that fits the project and implement there. The HTML uses inline styles only because of the authoring tool; do not carry that pattern into production.

## Fidelity
**High-fidelity.** Final colors, typography, spacing and states. Recreate pixel-accurately with the codebase's own primitives. Interaction behavior is described below but is not implemented in the mock — the mock is static.

## Screens / Views

### 1a — Inbox (three panes)
**Purpose:** scan and triage mail.
**Window:** 1180 × 720, radius 12, background #faf8f5.

**Title bar** — height 46, background #f2efe9, 1px bottom border rgba(0,0,0,.07), horizontal padding 14.
- Traffic lights: three 12px circles, gap 8 — #ff5f57 / #febc2e / #28c840.
- Vertical divider 1 × 20, rgba(0,0,0,.09).
- "New message" button: padding 5/10, radius 7, background #faf8f5, 1px border rgba(0,0,0,.09), label 12px/500 #1b1917, leading 13px envelope icon stroked #2c5fb8 at 1.4.
- Icon group (archive, trash, reply): 16px stroke icons, #5c564f at 1.4, gap 12.
- Search field right-aligned: width 230, padding 5/10, radius 7, background rgba(0,0,0,.045), 12px placeholder #8a837b with 13px magnifier.

**Sidebar** — width 206, background #f2efe9, right border rgba(0,0,0,.07), padding 16/10, item gap 2.
- Section label: 10px/600, uppercase, letter-spacing .09em, #a09890, padding 0 8 8.
- Item: padding 7/8, radius 7, gap 9, 13px/500 #3f3a35, 15px stroke icon #6f6963.
- Selected item ("Inbox"): background #d7e2f4, weight 600, #1b1917, icon stroke #2c5fb8, right-aligned count 11px/600 #2c5fb8.
- Unselected count: 11px/500 #a09890.
- Mailboxes: Inbox (12), Sent, Drafts (2), Archive, Trash. Accounts: Personal (dot #2c5fb8), Studio (dot #7a8f6d) — 7px dots.

**Message list** — width 336, right border rgba(0,0,0,.07), background #faf8f5.
- Header: padding 13/16/11, "Inbox" 14px/600 #1b1917, right "12 unread" 11px/400 #a09890, bottom border.
- Row: display flex, gap 10, padding 12/16, bottom border rgba(0,0,0,.055).
  - Unread dot: 7px circle #2c5fb8, margin-top 5. Read rows reserve the same 7px of width, empty.
  - Line 1: sender (unread 13px/600 #1b1917; read 13px/500 #3f3a35) with timestamp 11px/400 #8a837b pushed right, baseline aligned.
  - Line 2: subject (unread 12.5px/500 #1b1917; read 12.5px/400 #3f3a35), single line, ellipsis.
  - Line 3: preview 12px/1.45 (unread #6f6963; read #8a837b), clamped to 2 lines.
  - Selected row: background #e2eaf7 plus `inset 3px 0 0 #2c5fb8` left rail.
- List scrolls; only the header is fixed.

**Reading pane** — flex:1, background #faf8f5.
- Header: padding 22/34/16, bottom border. Subject 19px/600, line-height 1.3, letter-spacing -.01em, #1b1917.
- Sender row (margin-top 14, gap 11): 32px avatar circle #d7e2f4 with 12px/600 #2c5fb8 initials; name 13px/600 #1b1917; meta "to me · 9:14 AM" 11.5px/400 #8a837b; right side Reply and Forward buttons — padding 6/12, radius 7, 1px border rgba(0,0,0,.12), 12px/500 #3f3a35.
- Body: padding 26/34, **Newsreader 15px / line-height 1.7 / #241f1b**, paragraph margin-bottom 16.
- Attachments: row of chips, gap 10, margin-top 26 — padding 9/13, radius 8, background #f2efe9, 1px border rgba(0,0,0,.07), 16px document icon stroked #2c5fb8; filename 12px/500 #1b1917 over size 10.5px/400 #8a837b.

### 1b — Reading a thread
**Purpose:** read one conversation with the list dismissed.
**Window:** 900 × 720. Same title bar treatment; toolbar is back-chevron, archive, trash, download (16px, #5c564f, gap 16) with "3 messages" 12px/500 #8a837b right-aligned.
- Thread header: padding 26/48/18, bottom border. Subject 24px/600, line-height 1.25, letter-spacing -.015em. Below it a mailbox pill — padding 3/9, radius 5, background #e2eaf7, 10.5px/600 #1d4489, letter-spacing .03em — and participant list 12px/400 #8a837b.
- Collapsed earlier messages: rows of padding 16/48, background #f6f3ee, bottom border rgba(0,0,0,.055) — 28px avatar, name 12.5px/500 #3f3a35, snippet 12.5px/400 #8a837b ellipsed, date 11.5px/400 #a09890 right.
- Expanded message: padding 24/48/30. 34px avatar, name 13.5px/600, meta 11.5px #8a837b. Primary "Reply" button is filled #2c5fb8 with #fff 12px/500 text, radius 7, padding 6/12; "Forward" is the outlined variant.
- Body: Newsreader 16px / 1.75 / #241f1b, max-width 62ch.
- Inline reply affordance at the bottom: padding 14/16, radius 9, background #fff, 1px border rgba(0,0,0,.1), 28px avatar, placeholder "Write a reply…" 13px #a09890, disabled Send chip (background #f2efe9, text #a09890).

### 1c — Compose
**Purpose:** write and send.
**Window:** 620 × 520.
- Title bar 44px: traffic lights, "New message" 12.5px/600 #5c564f, right side link icon 16px #5c564f and filled Send button — padding 5/13, radius 7, background #2c5fb8, 12px/600 #fff.
- Header fields, padding 0 20, each row separated by 1px rgba(0,0,0,.07): label column 50px wide, 12px/500 #a09890.
  - From: value 13px/400 #3f3a35 plus 11px chevron.
  - To: recipient token — padding 3/9/3/4, radius 20, background #e2eaf7, 12px/500 #1d4489, 18px avatar circle; caret; "Cc" affordance right-aligned 12px/500 #a09890.
  - Subject: 13.5px/500 #1b1917.
- Body: padding 22/20, Newsreader 15px / 1.7 / #241f1b, 1.5px caret in #2c5fb8.
- Format bar: padding 11/20, top border, background #f6f3ee, gap 18 — list, B, I, link icons #6f6963; right "Draft saved" 11.5px/400 #a09890.

### 1d — Settings
**Purpose:** everything configurable, on one panel, no tabs.
**Window:** 620 × 600. Title bar 44px with centered "Settings" 12.5px/600 #5c564f.
- Content padding 24/28, section gap 22.
- Section label: 10px/600 uppercase, letter-spacing .09em, #a09890, margin-bottom 10.
- Group card: 1px border rgba(0,0,0,.09), radius 9, background #fff; rows padding 12/14 divided by 1px rgba(0,0,0,.06).
- Accounts: status dot 8px (#2c5fb8 Personal, #7a8f6d Studio), name 13px/500 #1b1917, address 11.5px/400 #8a837b, protocol "IMAP" 11.5px #a09890 right. Below the card, "Add account" 12px/500 #2c5fb8.
- General rows: title 13px/500 #1b1917, optional helper 11.5px #8a837b.
  - Dropdown control: padding 4/9, radius 6, 1px border rgba(0,0,0,.12), 12px/500 #3f3a35, 10px chevron.
  - Toggle: 38 × 22, radius 11 — on #2c5fb8 with knob right, off #ddd8d0 with knob left; knob 18px #fff, `0 1px 2px rgba(0,0,0,.2)`, inset 2px.
  - Rows shown: Check for new mail (Every 5 minutes), Load remote images (off, with helper text), Group messages by thread (on), Undo send window (10 seconds).
- Signature: card with Newsreader 13px / 1.6 / #3f3a35 content.

## Interactions & Behavior
Not implemented in the mock; intended behavior:
- Selecting a list row loads it in the reading pane, clears the unread dot after ~1s dwell, and decrements the mailbox count.
- Collapsing the list (1b) is a toolbar toggle; the back chevron returns to 1a.
- Collapsed thread messages expand in place on click; the newest message is expanded by default.
- Reply opens the inline composer in 1b; New message and Forward open the 1c window.
- Archive and Trash act on the selection and show an undo affordance; Send honors the undo-send window from settings before dispatch.
- Hover: list rows tint to rgba(0,0,0,.03); outlined buttons darken their border to rgba(0,0,0,.2); filled buttons darken to #234e99. Focus: 2px #2c5fb8 ring at 40% alpha, offset 2.
- Keyboard: ↑/↓ move selection, ⌘R reply, ⌘⇧D send, ⌘⌫ trash, ⌘⇧A archive, ⌘F search.
- Empty and loading states are not designed yet.
- Fixed desktop window sizes; panes below ~900px window width should collapse the list to 1b's layout.

## State Management
- `accounts[]` (id, label, address, protocol, dotColor), `mailboxes[]` per account with unread counts.
- `selectedMailboxId`, `selectedThreadId`, `expandedMessageIds`, `listCollapsed`, `searchQuery`.
- `threads[]` → `messages[]` (sender, avatarInitials, to, timestamp, bodyHtml, attachments[]).
- `composeDrafts[]` with autosave, plus `pendingSends[]` for the undo-send window.
- `settings`: fetchInterval, loadRemoteImages, threadGrouping, undoSendSeconds, signature.
- Data fetching: IMAP/JMAP sync on the fetch interval; optimistic updates for read/archive/trash with rollback on failure.

## Design Tokens
Colors
- Accent #2c5fb8 · accent dark (text on tint) #1d4489 · accent tint #e2eaf7 · accent tint deep (avatars) #d7e2f4
- Ink #1b1917 · body serif ink #241f1b · secondary #3f3a35 · muted #6f6963 · soft #8a837b · faint #a09890
- Canvas #faf8f5 · chrome #f2efe9 · sunken row #f6f3ee · card #fff · desk background #e8e4dc
- Secondary account dot #7a8f6d
- Borders: rgba(0,0,0,.07) chrome · rgba(0,0,0,.055) list rows · rgba(0,0,0,.09) sidebar/cards · rgba(0,0,0,.12) button outline
- Traffic lights #ff5f57 / #febc2e / #28c840

Typography
- UI: Instrument Sans — 10 / 11 / 11.5 / 12 / 12.5 / 13 / 13.5 / 14 / 19 / 24 px; weights 400, 500, 600.
- Message body: Newsreader — 15 px / 1.7 (pane), 16 px / 1.75 (thread), 13 px / 1.6 (signature).
- Uppercase labels: 10px/600, letter-spacing .09em. Large headings use letter-spacing -.01 to -.015em.

Spacing — 2 / 5 / 7 / 9 / 10 / 12 / 14 / 16 / 20 / 22 / 26 / 34 / 48.
Radius — 5 (pill label), 6 (small control), 7 (button), 8 (chip), 9 (card), 11 (toggle), 12 (window), 50% (avatar/dot).
Shadows — window `0 24px 60px rgba(40,32,20,.18), 0 2px 6px rgba(40,32,20,.1)`; toggle knob `0 1px 2px rgba(0,0,0,.2)`; selected row rail `inset 3px 0 0 #2c5fb8`.

## Assets
No images. All icons are inline SVG placeholders drawn on a 16×16 grid with 1.4–1.5 stroke weight, `fill:none`, currentColor-equivalent hex #5c564f or #6f6963 — replace with the codebase's real icon set (envelope, archive, trash, reply, forward, back chevron, download, search, document, link, list, bold, italic, disclosure chevron). Fonts are Google Fonts: Instrument Sans (400/500/600/700) and Newsreader (400/500). All sender names, addresses and message copy are fictional sample content.

## Files
- `Mail Mockups.dc.html` — all four screens on one pan/zoom canvas, anchored as #1a (inbox), #1b (thread), #1c (compose), #1d (settings).
