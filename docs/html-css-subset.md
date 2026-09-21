# HTML email CSS subset

Snail deliberately implements a small, deterministic email CSS model. Sanitization happens before
layout, and unsupported declarations are discarded instead of guessed.

## Supported

- `color`, `background-color`, and a single `background-image: url(...)`
- `font-family`, `font-size`, `font-weight`, `font-style`, and the common `font` shorthand
- `text-align`, underline via `text-decoration`, and `line-height`
- `margin`, `margin-top`, `margin-bottom`, horizontal `auto` margins, and all four `padding` sides
- `border`, `border-width`, the four directional borders, and `border-color`
- `width`, `height`, and `max-width` in pixels or percentages
- `display: block | inline | inline-block | table | table-row-group | table-row | table-cell | none`
- `vertical-align: top | middle | bottom | baseline`
- `list-style` / `list-style-type`: `none`, `disc`, and `decimal`
- `visibility: hidden | collapse` as `display: none`, for hidden email preheaders

Legacy email attributes are resolved before CSS: `bgcolor`, `align`, `valign`, `width`, `height`,
`cellpadding`, `cellspacing`, `border`, `colspan`, `rowspan`, and `<font size color face>`.

## Intentionally unsupported

`float`, `position`, flexbox, grid, transforms, media queries, pseudo-elements, animation, and web
fonts are dropped. Selectors support the final tag/id/class compound used by common email templates;
Snail does not implement a general browser CSSOM.

The executable fixture is
[`crates/snail-ui/tests/fixtures/e6-css-subset.css`](../crates/snail-ui/tests/fixtures/e6-css-subset.css).
