//! Snail's GPUI app: views, models, theme application, actions/keymap (plan.md §2).

use gpui_kit::component::{Root, TitleBar};
use gpui_kit::*;

use snail_core::paths::Paths;

mod dev_overlay;
mod frame_stats;
mod rss;
mod shell;
mod startup;

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
