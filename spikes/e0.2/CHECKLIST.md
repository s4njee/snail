# E0.2 checklist — gpui-kit `Input`/`Textarea` under real load

Reproduce with `cd spikes/e0.2 && cargo run`. Record the result in `spikes/RESULTS.md`.

The window is a 620×520 compose mock: To / Cc / Subject single-line inputs above a multi-line body
pre-filled with 40 wrapped lines. Delivered events are shown in the strip at the bottom.

## Automated / code-level (verified by reading the 0.6.4 source)

- [ ] `Input::new(&Entity<InputState>)` and `Textarea::new(&Entity<TextareaState>)` compile and
      render.
- [ ] API surface recorded: `InputState = InputBaseState<InputMode>`,
      `TextareaState = InputBaseState<TextareaMode>`; both `::new(window, cx)`; builder
      `.placeholder(..)`, `.default_value(..)`, `.set_value(..)`, `.value()`, `.text()`,
      `.focus(window, cx)`, `.set_auto_grow(min,max,cx)`.
- [ ] `InputEvent` at 0.6.4 is `Change | PressEnter { secondary, shift } | Focus | Blur` — thin.
      (What the plan wrote against 0.6.1 is superseded; record this.)

## Interactive (needs a human at the keyboard)

- [ ] Tab moves To → Cc → Subject → body and back; focus is visibly correct.
- [x] Select-all / copy / paste / cut / undo / redo work in every field
      (`⌘A ⌘C ⌘V ⌘X ⌘Z ⇧⌘Z`, `Ctrl+…` elsewhere). **macOS confirmed 2026-09-21.**
- [ ] Placeholder shows when empty and disappears on first keystroke.
- [ ] Focus and Blur events appear in the event strip when tabbing in and out.
- [ ] The 40-line body soft-wraps rather than scrolling horizontally; caret movement at wrap works.
- [ ] **IME:** type with a CJK input method (e.g. Pinyin → 漢字) in Subject and in the body.
      Composition underline appears, candidates select, committed text is correct, no dropped or
      duplicated characters. Repeat for at least one other IME (Japanese or Korean).
- [ ] No panic when a hover style is applied (plan.md §1.2: GPUI panics if a hover style is set
      twice — confirm the component's own styling doesn't trip it).

## Cross-platform

- [ ] macOS: the above.
- [ ] Linux (E0.11 machine): the above. Needs a working Vulkan driver.
- [ ] Windows (E0.11 machine): the above, and `Ctrl` shortcuts.

## Verdict

- **Pass** if the 40-line wrapped body, tab order, clipboard/undo and IME all work.
- **Fail** if the `Textarea` cannot carry the body → fallback is a hand-written text element
  against `window.text_system()` (~3 weeks), and E7's estimate changes.
