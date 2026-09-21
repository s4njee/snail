//! The only place colours and text styles are turned into GPUI values (plan.md E1.3/E1.4/E1.7).
//!
//! Views call `style::text(..)` and read `palette(..)`; they never write a raw hex or a raw size.

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::prelude::*;
use gpui_kit::*;

use snail_ui::text::{self, Family, TextColor, TextRole};
use snail_ui::theme::{self, Color, Mode};

/// The active Snail palette, as a GPUI global, so `text` and `palette` need no plumbing.
struct ActivePalette(&'static theme::Theme);

impl Global for ActivePalette {}

/// Install both the Snail palette and the mapped gpui-kit `Theme`, before the window opens.
pub fn install(mode: Mode, cx: &mut App) {
    cx.set_global(ActivePalette(theme::Theme::by_mode(mode)));
    map_component_theme(mode, cx);
}

pub fn palette(cx: &App) -> &'static theme::Theme {
    cx.global::<ActivePalette>().0
}

pub fn color(token: Color) -> Hsla {
    rgba(color_u32(token)).into()
}

fn color_for(palette: &theme::Colors, role: TextColor) -> Color {
    match role {
        TextColor::Ink => palette.ink,
        TextColor::BodyInk => palette.body_ink,
        TextColor::Secondary => palette.secondary,
        TextColor::Muted => palette.muted,
        TextColor::Soft => palette.soft,
        TextColor::Faint => palette.faint,
        TextColor::Accent => palette.accent,
        TextColor::AccentText => palette.accent_text,
        TextColor::Inverted => [0xff, 0xff, 0xff, 0xff],
    }
}

pub fn color_u32(color: Color) -> u32 {
    (color[0] as u32) << 24 | (color[1] as u32) << 16 | (color[2] as u32) << 8 | color[3] as u32
}

/// The only way a view produces text (plan.md E1.4).
pub fn text(content: impl Into<SharedString>, role: TextRole, cx: &App) -> Div {
    let palette = palette(cx);
    let spec = text::spec(role);
    let raw: SharedString = content.into();
    let content = if spec.uppercase {
        raw.to_uppercase().into()
    } else {
        raw
    };
    div()
        .font_family(family(spec.family))
        .text_size(px(spec.size))
        .font_weight(weight(spec.weight))
        .line_height(px(spec.size * spec.line_height))
        .text_color(color(color_for(&palette.colors, spec.color)))
        .child(content)
}

fn family(family: Family) -> SharedString {
    match family {
        Family::Ui => "Instrument Sans".into(),
        Family::Serif => "Newsreader".into(),
        Family::Mono => "DM Mono".into(),
    }
}

fn weight(weight: u16) -> FontWeight {
    match weight {
        700 => FontWeight::BOLD,
        600 => FontWeight::SEMIBOLD,
        500 => FontWeight::MEDIUM,
        _ => FontWeight::NORMAL,
    }
}

/// Map Snail's palette onto gpui-kit's `Theme` so its components land on our colours, type scale
/// and radii (plan.md E1.7). Every value set here has a Snail token behind it.
fn map_component_theme(mode: Mode, cx: &mut App) {
    Theme::change(
        match mode {
            Mode::Light => ThemeMode::Light,
            Mode::Dark => ThemeMode::Dark,
        },
        None,
        cx,
    );
    let theme = Theme::global_mut(cx);
    let palette = theme::Theme::by_mode(mode);
    let c = &palette.colors;

    theme.font_family = "Instrument Sans".into();
    theme.mono_font_family = "DM Mono".into();
    theme.font_size = px(13.0);
    theme.mono_font_size = px(12.0);
    theme.radius = px(palette.radii.button);
    theme.radius_lg = px(palette.radii.window);

    let colors = &mut theme.colors;
    colors.background = color(c.canvas);
    colors.foreground = color(c.ink);
    colors.muted = color(c.sunken);
    colors.muted_foreground = color(c.muted);
    colors.border = color(c.border_soft);
    colors.accent = color(c.accent);
    colors.accent_foreground = color([0xff, 0xff, 0xff, 0xff]);
    colors.link = color(c.accent);
    colors.link_hover = color(c.accent_hover);
    colors.selection = color(c.accent_tint);
    colors.caret = color(c.caret);
    colors.ring = color(c.accent);

    colors.list = color(c.canvas);
    colors.list_hover = color([0x00, 0x00, 0x00, 0x08]);
    colors.list_active = color(c.accent_tint);
    colors.list_active_border = color(c.accent);
    colors.list_even = color(c.canvas);
    colors.list_head = color(c.sunken);

    colors.popover = color(c.card);
    colors.popover_foreground = color(c.ink);
    colors.input = color(c.card);
    colors.table = color(c.sunken);
    colors.table_even = color(c.card);
    colors.table_head = color(c.sunken);
    colors.table_head_foreground = color(c.secondary);
    colors.table_hover = color(c.accent_tint);

    colors.title_bar = color(c.chrome);
    colors.title_bar_border = color(c.border_hairline);
    colors.sidebar = color(c.chrome);
    colors.sidebar_foreground = color(c.secondary);
    colors.sidebar_border = color(c.border_soft);

    colors.button = color(c.card);
    colors.button_foreground = color(c.ink);
    colors.button_hover = color(c.accent_tint);
    colors.button_primary = color(c.accent);
    colors.button_primary_hover = color(c.accent_hover);
    colors.button_primary_active = color(c.accent_hover);
    colors.button_primary_foreground = color([0xff, 0xff, 0xff, 0xff]);
    colors.switch = color(c.toggle_off);
    colors.scrollbar_thumb = color(c.border);
}
