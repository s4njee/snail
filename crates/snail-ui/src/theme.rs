//! The token table from plan.md §1.3 (mail ground) and §1.3b (dark), as two palettes behind one set
//! of names. Views never write a raw hex or a raw text size; they read from here.
//!
//! Both palettes are the same *rank* — `desk < chrome < sunken < canvas < card` by elevation — so a
//! view never branches on the theme. The `#[test]`s below are the guard: a dark value can never be
//! silently missing (the struct makes that a compile error) and a text/surface pair cannot drift
//! below its contrast floor (E1.3b).

/// RGBA, 0..=255. Alpha is meaningful for borders, tints and the search fill.
pub type Color = [u8; 4];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Light,
    Dark,
}

/// Every colour the app uses, in both themes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colors {
    // Surfaces, ranked by elevation.
    pub desk: Color,
    pub chrome: Color,
    pub sunken: Color,
    pub canvas: Color,
    pub card: Color,

    // Text.
    pub ink: Color,
    pub body_ink: Color,
    pub secondary: Color,
    pub muted: Color,
    pub soft: Color,
    pub faint: Color,

    // Accent.
    pub accent: Color,
    pub accent_hover: Color,
    pub accent_tint: Color,
    pub accent_tint_deep: Color,
    pub accent_text: Color,

    // Borders (alpha ladder) and fills.
    pub border_hairline: Color,
    pub border_soft: Color,
    pub border: Color,
    pub border_strong: Color,
    pub search_fill: Color,

    // Controls and odds and ends.
    pub toggle_off: Color,
    pub caret: Color,
    pub account_dot_secondary: Color,

    // Calendar identity colours (unchanged in spirit; lifted for dark).
    pub classes: Color,
    pub personal: Color,
    pub work: Color,
    pub birthdays: Color,
    pub home: Color,
    pub offline: Color,
    pub classes_tint: Color,
    pub personal_tint: Color,
    pub work_tint: Color,
    pub birthdays_tint: Color,
    pub home_tint: Color,

    // Status (aliases of the calendar colours, per §1.3).
    pub danger: Color,
    pub ok: Color,
    pub syncing: Color,
}

pub const LIGHT: Colors = Colors {
    // Owner, 2026-09-21: neutral greys, not the handoff's warm stone. Each grey keeps the
    // luminance of the tone it replaced, so contrast and hierarchy are unchanged.
    desk: [0xe4, 0xe4, 0xe4, 0xff],
    chrome: [0xef, 0xef, 0xef, 0xff],
    sunken: [0xf3, 0xf3, 0xf3, 0xff],
    canvas: [0xf8, 0xf8, 0xf8, 0xff],
    card: [0xff, 0xff, 0xff, 0xff],

    ink: [0x19, 0x19, 0x19, 0xff],
    body_ink: [0x20, 0x20, 0x20, 0xff],
    secondary: [0x3b, 0x3b, 0x3b, 0xff],
    muted: [0x6a, 0x6a, 0x6a, 0xff],
    soft: [0x84, 0x84, 0x84, 0xff],
    faint: [0x99, 0x99, 0x99, 0xff],

    accent: [0x2c, 0x5f, 0xb8, 0xff],
    accent_hover: [0x23, 0x4e, 0x99, 0xff],
    accent_tint: [0xe2, 0xea, 0xf7, 0xff],
    accent_tint_deep: [0xd7, 0xe2, 0xf4, 0xff],
    accent_text: [0x1d, 0x44, 0x89, 0xff],

    border_hairline: [0x00, 0x00, 0x00, 14],
    border_soft: [0x00, 0x00, 0x00, 18],
    border: [0x00, 0x00, 0x00, 23],
    border_strong: [0x00, 0x00, 0x00, 31],
    search_fill: [0x00, 0x00, 0x00, 11],

    toggle_off: [0xd9, 0xd9, 0xd9, 0xff],
    caret: [0xc3, 0xc3, 0xc3, 0xff],
    account_dot_secondary: [0x7a, 0x8f, 0x6d, 0xff],

    classes: [0xc2, 0x41, 0x0c, 0xff],
    personal: [0x1f, 0x6f, 0xeb, 0xff],
    work: [0x0f, 0x76, 0x6e, 0xff],
    birthdays: [0x7c, 0x5c, 0xbf, 0xff],
    home: [0xa1, 0x62, 0x07, 0xff],
    offline: [0x88, 0x88, 0x88, 0xff],
    classes_tint: [0xc2, 0x41, 0x0c, 26],
    personal_tint: [0x1f, 0x6f, 0xeb, 26],
    work_tint: [0x0f, 0x76, 0x6e, 26],
    birthdays_tint: [0x7c, 0x5c, 0xbf, 26],
    home_tint: [0xa1, 0x62, 0x07, 26],

    danger: [0xc2, 0x41, 0x0c, 0xff],
    ok: [0x0f, 0x76, 0x6e, 0xff],
    syncing: [0xa1, 0x62, 0x07, 0xff],
};

pub const DARK: Colors = Colors {
    // Owner, 2026-09-21: a neutral dark-grey ground, not the warm brown §1.3b originally proposed.
    desk: [0x0a, 0x0a, 0x0a, 0xff],
    chrome: [0x1a, 0x1a, 0x1a, 0xff],
    sunken: [0x1e, 0x1e, 0x1e, 0xff],
    canvas: [0x12, 0x12, 0x12, 0xff],
    card: [0x26, 0x26, 0x26, 0xff],

    ink: [0xf2, 0xf2, 0xf2, 0xff],
    body_ink: [0xe6, 0xe6, 0xe6, 0xff],
    secondary: [0xc8, 0xc8, 0xc8, 0xff],
    muted: [0xa3, 0xa3, 0xa3, 0xff],
    soft: [0x8a, 0x8a, 0x8a, 0xff],
    faint: [0x6e, 0x6e, 0x6e, 0xff],

    accent: [0x5b, 0x8e, 0xe0, 0xff],
    accent_hover: [0x7a, 0xa5, 0xe8, 0xff],
    accent_tint: [0x1e, 0x2c, 0x42, 0xff],
    accent_tint_deep: [0x25, 0x34, 0x4d, 0xff],
    accent_text: [0x9c, 0xc0, 0xf2, 0xff],

    border_hairline: [0xff, 0xff, 0xff, 14],
    border_soft: [0xff, 0xff, 0xff, 18],
    border: [0xff, 0xff, 0xff, 23],
    border_strong: [0xff, 0xff, 0xff, 36],
    search_fill: [0xff, 0xff, 0xff, 13],

    toggle_off: [0x2e, 0x2e, 0x2e, 0xff],
    caret: [0x4d, 0x4d, 0x4d, 0xff],
    account_dot_secondary: [0x93, 0xa8, 0x84, 0xff],

    classes: [0xe2, 0x72, 0x3f, 0xff],
    personal: [0x5b, 0x93, 0xf0, 0xff],
    work: [0x2f, 0xa8, 0x9d, 0xff],
    birthdays: [0xa4, 0x8a, 0xd8, 0xff],
    home: [0xd0, 0x9a, 0x35, 0xff],
    offline: [0x83, 0x83, 0x83, 0xff],
    classes_tint: [0xe2, 0x72, 0x3f, 51],
    personal_tint: [0x5b, 0x93, 0xf0, 51],
    work_tint: [0x2f, 0xa8, 0x9d, 51],
    birthdays_tint: [0xa4, 0x8a, 0xd8, 51],
    home_tint: [0xd0, 0x9a, 0x35, 51],

    danger: [0xe2, 0x72, 0x3f, 0xff],
    ok: [0x2f, 0xa8, 0x9d, 0xff],
    syncing: [0xd0, 0x9a, 0x35, 0xff],
};

/// Corner radii, mail's scale plus the calendar's surviving micro-radii.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Radii {
    pub micro: f32,
    pub label: f32,
    pub control: f32,
    pub button: f32,
    pub chip: f32,
    pub card: f32,
    pub toggle: f32,
    pub window: f32,
    pub token: f32,
}

pub const RADII: Radii = Radii {
    micro: 2.0,
    label: 5.0,
    control: 6.0,
    button: 7.0,
    chip: 8.0,
    card: 9.0,
    toggle: 11.0,
    window: 12.0,
    token: 20.0,
};

/// Window and pane geometry (calendar geometry survives per §1.3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub window_w: f32,
    pub window_h: f32,
    pub window_min_w: f32,
    pub window_min_h: f32,
    pub titlebar_h: f32,
    pub sidebar_w: f32,
    pub list_w: f32,
    pub rail_w: f32,
    pub settings_nav_w: f32,
}

pub const METRICS: Metrics = Metrics {
    window_w: 1240.0,
    window_h: 820.0,
    window_min_w: 900.0,
    window_min_h: 600.0,
    titlebar_h: 52.0,
    sidebar_w: 232.0,
    list_w: 336.0,
    rail_w: 300.0,
    settings_nav_w: 236.0,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub y: f32,
    pub blur: f32,
    pub color: Color,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub mode: Mode,
    pub colors: Colors,
    pub radii: Radii,
    pub metrics: Metrics,
    pub window_shadow: [Shadow; 2],
    pub modal_shadow: Shadow,
}

pub const LIGHT_THEME: Theme = Theme {
    mode: Mode::Light,
    colors: LIGHT,
    radii: RADII,
    metrics: METRICS,
    window_shadow: [
        Shadow {
            y: 24.0,
            blur: 60.0,
            color: [40, 32, 20, 46],
        },
        Shadow {
            y: 2.0,
            blur: 6.0,
            color: [40, 32, 20, 26],
        },
    ],
    modal_shadow: Shadow {
        y: 24.0,
        blur: 60.0,
        color: [40, 32, 20, 87],
    },
};

pub const DARK_THEME: Theme = Theme {
    mode: Mode::Dark,
    colors: DARK,
    radii: RADII,
    metrics: METRICS,
    window_shadow: [
        Shadow {
            y: 24.0,
            blur: 60.0,
            color: [0, 0, 0, 140],
        },
        Shadow {
            y: 2.0,
            blur: 6.0,
            color: [0, 0, 0, 90],
        },
    ],
    // On dark, elevation is carried by the surface step and a white hairline, not a shadow.
    modal_shadow: Shadow {
        y: 24.0,
        blur: 60.0,
        color: [0, 0, 0, 90],
    },
};

impl Theme {
    pub fn by_mode(mode: Mode) -> &'static Theme {
        match mode {
            Mode::Light => &LIGHT_THEME,
            Mode::Dark => &DARK_THEME,
        }
    }
}

// --- Contrast (E1.3b) -----------------------------------------------------------------------

/// Blend `fg` over `bg` using `fg`'s alpha, so translucent borders and tints can be measured.
pub fn composite(fg: Color, bg: Color) -> Color {
    let alpha = fg[3] as f32 / 255.0;
    let blend =
        |index: usize| (fg[index] as f32 * alpha + bg[index] as f32 * (1.0 - alpha)).round() as u8;
    [blend(0), blend(1), blend(2), 0xff]
}

fn linear(channel: u8) -> f32 {
    let value = channel as f32 / 255.0;
    if value <= 0.039_28 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

pub fn luminance(color: Color) -> f32 {
    0.2126 * linear(color[0]) + 0.7152 * linear(color[1]) + 0.0722 * linear(color[2])
}

/// WCAG contrast ratio, treating `fg` as opaque-over-`bg` when it has alpha.
pub fn contrast(fg: Color, bg: Color) -> f32 {
    let fg = if fg[3] < 255 { composite(fg, bg) } else { fg };
    let (a, b) = (luminance(fg), luminance(bg));
    let (hi, lo) = if a > b { (a, b) } else { (b, a) };
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Text pairs, all of which must clear AA. `faint` and the decorative hairlines are handled by
    /// their own documented tests below rather than silently loosened here.
    fn text_pairs(colors: &Colors) -> Vec<(&'static str, Color, Color, f32)> {
        vec![
            ("ink/canvas", colors.ink, colors.canvas, 4.5),
            ("ink/card", colors.ink, colors.card, 4.5),
            ("ink/chrome", colors.ink, colors.chrome, 4.5),
            ("body_ink/canvas", colors.body_ink, colors.canvas, 4.5),
            ("secondary/canvas", colors.secondary, colors.canvas, 4.5),
            ("secondary/card", colors.secondary, colors.card, 4.5),
            ("muted/canvas", colors.muted, colors.canvas, 4.5),
            (
                "accent_text/accent_tint",
                colors.accent_text,
                colors.accent_tint,
                4.5,
            ),
            ("accent/canvas", colors.accent, colors.canvas, 4.5),
            ("ok/card", colors.ok, colors.card, 4.5),
            ("danger/card", colors.danger, colors.card, 4.5),
        ]
    }

    #[test]
    fn both_themes_meet_the_text_contrast_floor() {
        for (theme, name) in [(&LIGHT_THEME, "light"), (&DARK_THEME, "dark")] {
            for (pair, fg, bg, floor) in text_pairs(&theme.colors) {
                let ratio = contrast(fg, bg);
                assert!(ratio >= floor, "{name} {pair}: {ratio:.2} < {floor:.1}");
            }
        }
    }

    #[test]
    fn faint_is_a_known_low_contrast_tone() {
        // Documented, not guarded: the handoff's faint micro-label tone is below AA.
        for (theme, name) in [(&LIGHT_THEME, "light"), (&DARK_THEME, "dark")] {
            let ratio = contrast(theme.colors.faint, theme.colors.canvas);
            assert!(ratio < 4.5, "{name} faint unexpectedly passes: {ratio:.2}");
        }
    }

    #[test]
    fn hairlines_are_decorative_not_meaningful_boundaries() {
        // The handoff's dividers are deliberately faint (~1.2–1.6:1), below WCAG's 3:1 non-text
        // floor. That is acceptable only while a boundary never *carries* meaning on its own: the
        // outlined button in E1.9 paints a fill/label too, and any control whose sole cue is its
        // border must not use these tokens. This test pins the current values so a nudge is noticed.
        for (theme, name) in [(&LIGHT_THEME, "light"), (&DARK_THEME, "dark")] {
            let ratio = contrast(theme.colors.border, theme.colors.canvas);
            assert!(ratio < 3.0, "{name} border unexpectedly strong: {ratio:.2}");
        }
    }

    #[test]
    fn surfaces_are_ordered_for_their_theme() {
        // Light: the window ground sits between the chrome and the elevated card.
        let light = &LIGHT_THEME.colors;
        assert!(luminance(light.desk) < luminance(light.chrome));
        assert!(luminance(light.chrome) < luminance(light.canvas));
        assert!(luminance(light.sunken) < luminance(light.canvas));
        assert!(luminance(light.canvas) < luminance(light.card));

        // Dark: a neutral dark-grey ground, with desk below it and chrome/sunken/card above
        // (owner, 2026-09-21).
        let dark = &DARK_THEME.colors;
        assert!(
            luminance(dark.canvas) < 0.02,
            "dark canvas should be near-black"
        );
        assert!(luminance(dark.desk) < luminance(dark.canvas));
        assert!(luminance(dark.canvas) < luminance(dark.chrome));
        assert!(luminance(dark.chrome) < luminance(dark.sunken));
        assert!(luminance(dark.sunken) < luminance(dark.card));
    }

    #[test]
    fn borders_are_translucent() {
        for colors in [LIGHT, DARK] {
            assert!(colors.border[3] < 255);
            assert!(colors.search_fill[3] < 255);
            assert!(colors.personal_tint[3] < 255);
        }
    }

    #[test]
    fn report() {
        // Run with `-- --nocapture` to read the table.
        for (theme, name) in [(&LIGHT_THEME, "light"), (&DARK_THEME, "dark")] {
            for (pair, fg, bg, _) in text_pairs(&theme.colors) {
                eprintln!("{name:<5} {pair:<28} {:.2}", contrast(fg, bg));
            }
            eprintln!(
                "{name:<5} {:<28} {:.2}",
                "faint/canvas",
                contrast(theme.colors.faint, theme.colors.canvas)
            );
            eprintln!(
                "{name:<5} {:<28} {:.2}",
                "border/canvas",
                contrast(theme.colors.border, theme.colors.canvas)
            );
        }
    }
}
