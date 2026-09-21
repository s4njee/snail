# E0 — what I need from you to pass the spike gates

**Status 2026-09-21:** E0.1/E0.5/E0.7 pass. E0.2 builds and runs — needs your interactive/IME pass
(`spikes/e0.2/CHECKLIST.md`). E0.4 **passes on both accounts**; the E0.3 corpus is dumped (24
messages in gitignored `spikes/corpus/`). What remains is at the bottom: hardware, CI, and the
backfill-depth decision (now urgent — your Gmail is 669,873 messages).

M0 is "the three spikes pass" (plan.md §5). E0.1, E0.5, E0.7–E0.10 I can do without you. The three
gates themselves each need something only you have.

## Gate 1 — E0.3: the HTML corpus (the big one)

The spike is meaningless without real mail, and the plan is specific: *20 messages from your actual
inbox, deliberately weighted to the worst.* Required categories:

1. nested-table marketing newsletter
2. GitHub notification
3. Google Calendar invite
4. quoted-reply chain five levels deep
5. Apple Mail-generated reply
6. plain text with `format=flowed`
7. one with inline `cid:` images

…plus whatever else your inbox actually contains that looks hostile.

**Pick one:**

- **A (recommended): let me fetch them.** Once E0.4 has a Gmail token, I write a CLI that pulls raw
  MIME (`messages.get?format=raw`) for a set of search queries and saves `.eml` files into
  `spikes/corpus/`. You only approve scopes; no manual exporting. This also proves the E0.4 plumbing
  twice. iCloud messages (Apple Mail replies, `cid:`) come the same way over IMAP once the app
  password exists.
- **B: you drop files.** Put `.eml` files (or one `.mbox`) in `spikes/corpus/`. From Mail.app,
  select messages → File → Save As… (an `.mbox`/`.rtf`; plain `.eml` via drag to Finder works too).
  Raw MIME with attachments is what matters — not screenshots, not exported PDFs.
- **C: synthetic.** I generate worst-case fixtures for each category myself. Faster and private, but
  it tests the categories I *imagine*, not the ones you actually get, which weakens the gate.

## Gate 2 — E0.4: both accounts authenticate and pull one page

### Google (also E0.6)
1. Say which Google account is the daily driver.
2. In Google Cloud Console, create (or let me walk you through) a project with **Gmail API** and
   **Google Calendar API** enabled.
3. OAuth consent screen → **External** → then **Publish app** to **In production** (unverified).
   *Not Testing* — testing expires refresh tokens after 7 days (plan.md §6.2).
4. Credentials → **Create OAuth client ID → Desktop app**.
5. Give me the **client ID** and **client secret**. (The secret is not confidential for installed
   apps, but we'll inject it at build time rather than commit it — plan.md E3.1.)
6. Be at the keyboard once to click through the "Google hasn't verified this app → Advanced → Go to
   … (unsafe)" interstitial.
7. **Start the day-8 test now.** After first consent, we re-check the refresh token on day 8. Google
   changes this often, and the whole account strategy depends on the answer.

### iCloud
1. Confirm 2FA is on for the Apple Account.
2. Generate an **app-specific password** at **account.apple.com** → Sign-In and Security →
   App-Specific Passwords (plan.md E3.4).
3. Give me the Apple ID address and the app password; I'll put the password in the keychain, never
   in the store or a JSON file.

### Probes the accounts unlock (E0.4 / §6.7–6.8)
- Fresh iCloud IMAP capability dump, and whether CONDSTORE/QRESYNC actually *function*.
- Whether iCloud files the sent copy server-side (→ whether we APPEND).
- The SMTP status code on quota exceedance.
- Whether iCloud supports RFC 6578 `sync-collection` (the plan's "highest-value ten-minute test").
- Whether `ATTENDEE` PUT double-sends invitations.

## Gate 3 — E0.11: hardware for the cross-platform half

- **Linux box with a working Vulkan driver** (hard requirement, plan.md §1.2) — Ferrite used a Pi 5
  on labwc. Is that still reachable, or is there another Linux machine?
- **Windows machine** — even a VM. GPUI needs a real GPU; RDP/VDI will not work.
- Tell me how to reach them (ssh host/alias) and I'll get a checkout building there.

## E0.10 — CI / repo

`gh` is authenticated as **s4njee** with `repo` + `workflow` scopes, but `snail` has no git remote.
**Do you want me to create and push a GitHub repo?** If so, name and visibility (private
recommended for a personal tool that will hold an OAuth client secret at build time).

## Decisions that shape E0 outputs (not blockers)

1. **Backfill depth** (open question 3): everything, or last N years? Sets the realistic size of the
   E0.9 fixture and how much E0.4 pulls.
2. **Attachments** (open question 2): eager vs lazy — changes E0.9 fixture composition.
3. **Windows "kept green"** (open question 1): compiles/launches/basic flows, or the same budgets as
   Linux?
4. **The "Other platform" note** (open question 4) never made it into the plan — did it exist?

## Notes for the record (E0 findings so far)

- **gpui-kit 0.6.4 on gpui-pre 0.3.5** is the newest pair and resolves cleanly (plan.md §1.1
  predicted the bump). E0.2 is where it gets frozen — or rejected.
- **0.6.4 moved the editing engine into `gpui-base`**: `InputState = InputBaseState<InputMode>`,
  `TextareaState = InputBaseState<TextareaMode>`, and `InputEvent` is now only
  `Change | PressEnter | Focus | Blur`. This supersedes the 0.6.1 surface the plan was written
  against; it is thinner, and E7 should be planned against *this* one.
- **gpui-kit 0.6.4 ships `TextView::html(...)`** (`component::text::{html, markdown, TextView}`),
  backed by `gpui-base`. This is exactly the E6.13b shortcut the plan hoped for; E6 should evaluate
  it *first*, as a renderer and/or as 6.13's fallback.
