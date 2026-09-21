//! Snail's GPUI app: views, models, theme application, actions/keymap (plan.md §2).

use std::borrow::Cow;

use gpui_kit::component::{Root, TitleBar};
use gpui_kit::*;

use snail_core::paths::Paths;

mod dev_overlay;
mod frame_stats;
mod rss;
mod shell;
mod startup;
mod style;

fn main() {
    startup::begin();

    // Identity and paths first, so every later log line lands in the file and nothing ever touches
    // the real mailbox when SNAIL_CONFIG_DIR / SNAIL_CACHE_DIR point at a fixture (plan.md E0.5).
    let paths = Paths::resolve();
    let _ = paths.ensure();
    let _ = snail_core::paths::init_logging(&paths);
    log::info!("snail starting; config={:?} cache={:?}", paths.config, paths.cache);
    startup::mark("paths_and_logging");

    // Without the asset source every gpui-kit icon, including the Windows/Linux window controls,
    // silently paints nothing (plan.md §1.2, E1.1).
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx| {
            startup::mark("app_launched");
            gpui_kit::init(cx);
            startup::mark("gpui_init");

            // Fonts must be registered before the window opens or the first frame falls back to the
            // system face (plan.md E1.1). Self-hosted, bundled, no runtime fetch.
            cx.text_system()
                .add_fonts(bundled_fonts())
                .expect("the bundled Instrument Sans / Newsreader / DM Mono load");
            startup::mark("fonts");

            style::install(snail_ui::theme::Mode::Light, cx);
            startup::mark("component_theme");

            let bounds = Bounds::centered(None, size(px(1240.), px(820.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(900.), px(600.))),
                    // Otherwise labwc and other compositors draw their own title bar over ours
                    // (plan.md §1.2, E1.2).
                    window_decorations: Some(WindowDecorations::Client),
                    ..TitleBar::window_options()
                },
                |window, cx| {
                    startup::mark("window_opened");
                    // `Root` as the shell's parent so popovers, dialogs and context menus have
                    // somewhere to render (plan.md E1.1).
                    let shell = cx.new(|cx| shell::Shell::new(window, cx));
                    startup::mark("shell_built");
                    let root = cx.new(|cx| Root::new(shell, window, cx));
                    startup::mark("root_built");
                    // The first real frame, measured off the window rather than guessed.
                    window.on_next_frame(|_window, _cx| startup::mark("first_frame"));
                    root
                },
            )
            .expect("the main window opens");

            cx.activate(true);
        });
}

/// The bundled faces (plan.md §1.3): Instrument Sans (UI), Newsreader (message and event bodies),
/// DM Mono (times, dates, counts, labels). Self-hosted under the OFL; see `assets/fonts/LICENSES.md`.
fn bundled_fonts() -> Vec<Cow<'static, [u8]>> {
    vec![
        Cow::Borrowed(include_bytes!("../assets/fonts/InstrumentSans-400.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/InstrumentSans-500.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/InstrumentSans-600.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/InstrumentSans-700.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/Newsreader-400.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/Newsreader-500.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/DMMono-Regular.ttf")),
        Cow::Borrowed(include_bytes!("../assets/fonts/DMMono-Medium.ttf")),
    ]
}
