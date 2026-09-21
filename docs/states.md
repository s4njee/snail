# State matrix (plan.md E1.9)

Neither handoff draws a single hover, focus, empty, loading, error or disabled state. This table is
the contract for every interactive component, **with a dark column**, so E5/E6 build against a
decided design rather than inventing one per screen. It is a design artifact: review it before E5.

Values are Snail tokens (`snail-ui::theme`) except where a raw value is the specification. Where a
state changes with the theme, the light and dark columns differ.

## Shared values

| Element | Light | Dark |
|---|---|---|
| Row / list hover | `rgba(0,0,0,.03)` | `rgba(255,255,255,.04)` |
| Filled button hover | darken to `accent_hover` `#234e99` | **lighten** to `accent_hover` `#7aa5e8` |
| Outlined button border hover | `rgba(0,0,0,.2)` | `rgba(255,255,255,.2)` |
| Focus ring | 2px `accent` at 40% alpha, offset 2 | same token |
| Disabled | 40% opacity, no hover, `cursor: default` | same |
| Active / pressed | filled: `accent_hover`; outlined: `rgba(0,0,0,.06)` fill | filled: `accent`; outlined: `rgba(255,255,255,.08)` |
| Dragging (calendar) | 0.6 opacity on the ghost; 2px `accent` outline on the target | same |
| Motion | 120–180ms ease-out for view changes and sheet entry only | same |

## Components

| Component | Default | Hover | Focus | Selected / active | Disabled |
|---|---|---|---|---|---|
| Sidebar item | `SidebarItem` (`secondary`) | `rgba(0,0,0,.03)` fill | focus ring inside | `accent_tint_deep` fill, `SidebarItemSelected`, icon `accent` | 40% opacity |
| Message row (read) | canvas | row hover | focus ring; **focused-not-selected** = `rgba(0,0,0,.02)` fill, no rail | `accent_tint` + 3px `accent` inset rail | — |
| Message row (unread) | 7px `accent` dot | row hover | as above | as above; dot stays | — |
| Message row (multi-selected) | — | row hover | — | `accent_tint` on every selected row, rail only on the anchor | — |
| Message row (pending op) | — | — | — | preview replaced by "Archiving…", 60% opacity | — |
| Message row (send failed) | — | — | — | `danger` dot + "Not sent · Retry" in `danger` | — |
| Toolbar icon button | icon `muted` | icon `ink`, `rgba(0,0,0,.03)` fill, r7 | focus ring | pressed: `rgba(0,0,0,.06)` | icon `faint`, no hover |
| Filled button | `accent` fill, `ButtonLabelFilled` | `accent_hover` | focus ring | `accent_hover` | 40% opacity |
| Outlined button | border `border_strong`, `ButtonLabel` | border `rgba(0,0,0,.2)` | focus ring | `rgba(0,0,0,.06)` fill | 40% opacity |
| Text input | `card` fill, border `border_soft`, caret `caret` | border `border` | 2px `accent` ring, border `accent` | selection `accent_tint` | 40% opacity, `sunken` fill |
| Search field | `search_fill`, placeholder `soft` | fill `rgba(0,0,0,.06)` | 2px `accent` ring | — | — |
| Checkbox / Toggle | off track `toggle_off` | track +4% | focus ring | on: `accent` track, knob right | 40% opacity |
| Segmented control (titlebar) | transparent, label `muted` | label `secondary` | focus ring | active segment `card` fill + hairline + shadow-sm | 40% opacity |
| Tab | label `muted` | label `secondary` | focus ring | label `ink`, 2px `accent` underline | 40% opacity |
| Link | `accent`, underline on hover | `accent_hover`, underline | focus ring | — | `faint` |
| Dropdown / Select | like text input | fill `rgba(0,0,0,.03)` | focus ring | open: border `accent` | 40% opacity |
| Context menu / Popover | `card` fill, hairline, shadow | item hover `rgba(0,0,0,.03)` | focus ring on item | item selected `accent_tint` | item 40% opacity |
| Dialog / Sheet | `card`, scrim `rgba(0,0,0,.34)` | — | ring on default button | — | — |
| Month cell | `card` | `rgba(0,0,0,.02)` | focus ring | selected `accent_tint`; **today** solid `accent` pill | — |
| Day cell / hour row | canvas | `rgba(0,0,0,.02)` | focus ring | today wash `canvas`→ accent column | — |
| Mini-month day | canvas | `rgba(0,0,0,.03)` | focus ring | selected `accent_tint`; today solid `accent` | out-of-month `faint` |
| Calendar event block | 10% tint of its calendar colour | +4% alpha, shadow-sm | focus ring | selected: 2px `accent` outline | — |
| Attachment chip | `chrome` fill, hairline | fill `sunken` | focus ring | — | 40% opacity |
| Recipient token | `accent_tint`, `accent_text` | fill `accent_tint_deep` | focus ring; selected token shows a `×` | — | invalid: `danger` border + `danger` text |
| Scrollbar thumb | `border` | `rgba(0,0,0,.2)` | — | — | — |
| Undo bar | `ink` fill, `#fff` text | action label underline | focus ring | countdown bar `accent` | — |

## Loading, empty and error (E1.10)

Drawn from `snail_ui::empty`. One component, an icon + title + body, centred in its pane:
no accounts yet, mailbox syncing for the first time, empty mailbox, no search results, message body
failed to parse, offline, sync error, calendar range with no events. **Offline is not an error
state** and its copy says so (plan.md §2). Inline variants:

- **List loading:** three skeleton rows at the fixed row height, `sunken` fill, no shimmer.
- **Body loading:** parse happens off-thread; show the plain-text preview as a stand-in, never a spinner.
- **Sync error:** sidebar footer dot `danger` + "Couldn't sync" on click; the mailbox stays usable.
- **Dead-letter op:** row shows "Not sent · Retry" with an explicit retry, never a silent loop.

## Motion (E1.11)

Only two motions exist: view changes and sheet entry, both 120–180ms ease-out. **No row transitions,
no hover fades, no list animation.** Hover is instant by rule.
