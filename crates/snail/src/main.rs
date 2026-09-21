//! Snail's GPUI app: views, models, theme application, actions/keymap (plan.md §2).

use std::borrow::Cow;
use std::time::Instant;

use gpui_kit::component::Root;
use gpui_kit::*;

use snail_core::fixture::FixtureSpec;
use snail_core::paths::Paths;
use snail_core::providers::gmail::{FixedToken, GmailClient};
use snail_core::store::{NewMessage, ProviderRef, Store};
use snail_services::auth::GoogleOAuth;
use std::sync::Arc;

mod dev_overlay;
mod compose;
mod frame_stats;
mod html_view;
mod icons;
mod mail_model;
mod rss;
mod settings;
mod shell;
mod startup;
mod style;
mod tracked;

fn main() {
    startup::begin();

    // Identity and paths first, so every later log line lands in the file and nothing ever touches
    // the real mailbox when SNAIL_CONFIG_DIR / SNAIL_CACHE_DIR point at a fixture (plan.md E0.5).
    let paths = Paths::resolve();
    let _ = paths.ensure();
    let _ = snail_core::paths::init_logging(&paths);
    log::info!(
        "snail starting; config={:?} cache={:?}",
        paths.config,
        paths.cache
    );
    startup::mark("paths_and_logging");

    let theme_pref = settings::load(&paths);
    let settings_store = snail_core::settings::SettingsStore::new(&paths);

    // One store, opened before the window. A failure falls back to in-memory so the UI still opens.
    let store = match Store::open(&paths) {
        Ok(store) => Arc::new(store),
        Err(error) => {
            log::warn!("could not open the store ({error:#}); using an in-memory one");
            Arc::new(Store::open_in_memory().expect("in-memory store"))
        }
    };

    // Benchmark and fixture modes never open a window (E0.9, E2.9, E17.1).
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| {
        arg == "--generate-fixture"
            || arg == "--bench-store"
            || arg == "--backfill-gmail"
            || arg == "--import-eml"
            || arg == "--drain-sends"
    }) {
        if let Err(error) = run_store_cli(&paths, &args) {
            eprintln!("{error:#}");
            std::process::exit(1);
        }
        std::process::exit(0);
    }
    if args.iter().any(|arg| arg == "--fixture") {
        let store = Store::open(&paths).expect("open the fixture store");
        let count: i64 = store
            .with_db(|conn| {
                Ok(conn.query_row("SELECT count(*) FROM message", [], |row| row.get(0))?)
            })
            .unwrap_or(0);
        if count == 0 {
            let report = snail_core::fixture::generate(&store, &FixtureSpec::default())
                .expect("generate the fixture");
            log::info!("generated fixture: {} messages", report.messages);
        }
    }

    // Without the asset source every gpui-kit icon, including the Windows/Linux window controls,
    // silently paints nothing (plan.md §1.2, E1.1).
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            startup::mark("app_launched");
            gpui_kit::init(cx);
            startup::mark("gpui_init");

            // Fonts must be registered before the window opens or the first frame falls back to the
            // system face (plan.md E1.1). Self-hosted, bundled, no runtime fetch.
            cx.text_system()
                .add_fonts(bundled_fonts())
                .expect("the bundled Instrument Sans / Newsreader / DM Mono load");
            startup::mark("fonts");

            settings::install(settings_store.clone(), theme_pref, cx);
            startup::mark("component_theme");
            let mail = mail_model::MailModel::new(store.clone());
            let bounds = Bounds::centered(None, size(px(1240.), px(820.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(900.), px(600.))),
                    // Otherwise labwc and other compositors draw their own title bar over ours
                    // (plan.md §1.2, E1.2).
                    window_decorations: Some(WindowDecorations::Client),
                    // macOS draws the real traffic lights over a transparent 52px titlebar; the
                    // shell owns dragging (E1.2). Windows/Linux draw their own controls.
                    titlebar: Some(TitlebarOptions {
                        title: None,
                        appears_transparent: true,
                        traffic_light_position: Some(point(px(14.0), px(20.0))),
                        ..Default::default()
                    }),
                    app_owns_titlebar_drag: true,
                    ..Default::default()
                },
                |window, cx| {
                    startup::mark("window_opened");
                    // `Root` as the shell's parent so popovers, dialogs and context menus have
                    // somewhere to render (plan.md E1.1).
                    let shell = cx.new(|cx| shell::Shell::new(mail, window, cx));
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

/// The fixture generator and store benchmarks, for `--generate-fixture` and `--bench-store`.
fn env_var(name: &str) -> anyhow::Result<String> {
    std::env::var(name).map_err(|_| anyhow::anyhow!("{name} is not set"))
}

fn run_store_cli(paths: &Paths, args: &[String]) -> anyhow::Result<()> {
    let count = |flag: &str| -> Option<usize> {
        args.iter()
            .position(|arg| arg == flag)
            .and_then(|index| args.get(index + 1))
            .and_then(|value| value.parse().ok())
    };

    if args.iter().any(|arg| arg == "--generate-fixture") {
        let spec = FixtureSpec {
            messages: count("--messages").unwrap_or(200_000),
            events: count("--events").unwrap_or(20_000),
            ..FixtureSpec::default()
        };
        let store = Store::open(paths)?;
        let report = snail_core::fixture::generate(&store, &spec)?;
        println!(
            "fixture: {} accounts, {} mailboxes, {} messages, {} events in {} ms",
            report.accounts, report.mailboxes, report.messages, report.events, report.elapsed_ms
        );
    }

    if args.iter().any(|arg| arg == "--backfill-gmail") {
        // A dev command: reads the OAuth client and refresh token from the environment so nothing
        // secret is ever an argument or a committed file. Proves E4.2/E4.3/E4.22 end to end.
        let client_id = env_var("SNAIL_GOOGLE_CLIENT_ID")?;
        let client_secret = env_var("SNAIL_GOOGLE_CLIENT_SECRET")?;
        let refresh_token = env_var("SNAIL_GOOGLE_REFRESH_TOKEN")?;
        let oauth = GoogleOAuth::new(client_id, client_secret);
        let tokens = oauth.refresh(&refresh_token)?;

        let client = GmailClient::new(Arc::new(FixedToken(tokens.access_token)))?;
        let profile = client.profile()?;
        println!(
            "authenticated as {} ({} messages)",
            profile.email, profile.messages_total
        );

        let store = Store::open(paths)?;
        let account_id = match store.account_by_address("gmail", &profile.email)? {
            Some(account) => account.id,
            None => store.insert_account("gmail", &profile.email, None, 0)?,
        };
        let limit = count("--limit").unwrap_or(200);
        let report = client.backfill(&store, account_id, None, 30, limit)?;
        println!(
            "backfill: enumerated {}, inserted {}, head {:?}",
            report.enumerated, report.inserted, report.head_history_id
        );
    }

    if args.iter().any(|arg| arg == "--import-eml") {
        let path = args
            .iter()
            .position(|arg| arg == "--import-eml")
            .and_then(|index| args.get(index + 1))
            .ok_or_else(|| anyhow::anyhow!("--import-eml needs a path"))?;
        let bytes = std::fs::read(path)?;
        let parsed = snail_core::mime::parse_raw(&bytes)?;
        let store = Store::open(paths)?;
        let account_id = match store.account_by_address("local", "import@local")? {
            Some(account) => account.id,
            None => store.insert_account("local", "import@local", Some("Imported"), 0)?,
        };
        let mailbox = store.ensure_mailbox(account_id, "Inbox", "inbox")?;
        let thread = store.ensure_thread(account_id, None, parsed.subject.as_deref())?;
        let raw_hash = store.cache().put(&bytes)?;
        let inserted = store.insert_message_if_new(&NewMessage {
            account_id,
            mailbox_id: Some(mailbox),
            thread_id: Some(thread),
            provider: Some(ProviderRef::Gmail {
                id: format!("import-{raw_hash}"),
                thread_id: None,
                history_id: None,
            }),
            message_id: parsed.message_id,
            in_reply_to: parsed.in_reply_to,
            references: parsed.references,
            subject: parsed.subject,
            from_name: parsed.from_name,
            from_addr: parsed.from_addr,
            to_json: Some(serde_json::to_string(&parsed.to)?),
            date: parsed.date,
            preview: parsed.preview,
            unread: true,
            raw_hash: Some(raw_hash),
            ..Default::default()
        })?;
        store.recount_mailbox(mailbox)?;
        println!("imported {path}: inserted={inserted}");
    }

    if args.iter().any(|arg| arg == "--drain-sends") {
        let client_id = env_var("SNAIL_GOOGLE_CLIENT_ID")?;
        let client_secret = env_var("SNAIL_GOOGLE_CLIENT_SECRET")?;
        let refresh_token = env_var("SNAIL_GOOGLE_REFRESH_TOKEN")?;
        let oauth = GoogleOAuth::new(client_id, client_secret);
        let tokens = oauth.refresh(&refresh_token)?;
        let client = GmailClient::new(Arc::new(FixedToken(tokens.access_token)))?;
        let store = Store::open(paths)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0);
        let report = snail_services::outbox::drain_sends(&store, &client, 20, now)?;
        println!(
            "send drain: {} sent, {} failed, {} dead",
            report.sent, report.failed, report.dead
        );
    }

    if args.iter().any(|arg| arg == "--bench-store") {
        let started = Instant::now();
        let store = Store::open(paths)?;
        let cold_open_ms = started.elapsed().as_secs_f64() * 1000.0;
        let mut report = snail_core::fixture::benchmark(&store)?;
        report.cold_open_ms = cold_open_ms;
        println!("store bench ({} messages):", report.messages);
        println!("  cold open        {:>8.1} ms", report.cold_open_ms);
        println!(
            "  mailbox page     {:>8.2} / {:>8.2} ms  (p50 / p99)",
            report.page_p50_ms, report.page_p99_ms
        );
        println!("  thread assemble  {:>8.2} ms", report.thread_assemble_ms);
        println!("  unread counts    {:>8.2} ms", report.unread_counts_ms);
    }
    Ok(())
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
