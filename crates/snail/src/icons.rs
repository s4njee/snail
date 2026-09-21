//! Icon set as raw SVG bytes (plan.md E1.6), not gpui-kit's `Icon`, which fixes its colour at render
//! and so cannot be re-tinted on hover.
//!
//! The SVGs are Lucide (ISC), 24×24, `stroke="currentColor"`. Drawn at 16px, which scales the 2px
//! stroke to ~1.33 — close to the handoff's 1.4–1.5. Tint with `.text_color(..)`.

use gpui_kit::prelude::*;
use gpui_kit::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Envelope,
    Archive,
    Trash,
    Reply,
    Forward,
    Back,
    ChevronRight,
    ChevronDown,
    Download,
    Search,
    Document,
    Link,
    List,
    Bold,
    Italic,
    Inbox,
    Send,
    Calendar,
    Clock,
    Repeat,
    Bell,
    Plus,
    Close,
    Overflow,
    Lock,
    Shield,
}

/// The icon element, 16×16, tinted by the caller's text colour.
pub fn icon(name: Icon) -> Svg {
    svg().data(bytes(name)).w(px(16.0)).h(px(16.0))
}

fn bytes(name: Icon) -> &'static [u8] {
    match name {
        Icon::Envelope => include_bytes!("../assets/icons/mail.svg"),
        Icon::Archive => include_bytes!("../assets/icons/archive.svg"),
        Icon::Trash => include_bytes!("../assets/icons/trash.svg"),
        Icon::Reply => include_bytes!("../assets/icons/reply.svg"),
        Icon::Forward => include_bytes!("../assets/icons/forward.svg"),
        Icon::Back => include_bytes!("../assets/icons/chevron-left.svg"),
        Icon::ChevronRight => include_bytes!("../assets/icons/chevron-right.svg"),
        Icon::ChevronDown => include_bytes!("../assets/icons/chevron-down.svg"),
        Icon::Download => include_bytes!("../assets/icons/download.svg"),
        Icon::Search => include_bytes!("../assets/icons/search.svg"),
        Icon::Document => include_bytes!("../assets/icons/file.svg"),
        Icon::Link => include_bytes!("../assets/icons/link.svg"),
        Icon::List => include_bytes!("../assets/icons/list.svg"),
        Icon::Bold => include_bytes!("../assets/icons/bold.svg"),
        Icon::Italic => include_bytes!("../assets/icons/italic.svg"),
        Icon::Inbox => include_bytes!("../assets/icons/inbox.svg"),
        Icon::Send => include_bytes!("../assets/icons/send.svg"),
        Icon::Calendar => include_bytes!("../assets/icons/calendar.svg"),
        Icon::Clock => include_bytes!("../assets/icons/clock.svg"),
        Icon::Repeat => include_bytes!("../assets/icons/repeat.svg"),
        Icon::Bell => include_bytes!("../assets/icons/bell.svg"),
        Icon::Plus => include_bytes!("../assets/icons/plus.svg"),
        Icon::Close => include_bytes!("../assets/icons/x.svg"),
        Icon::Overflow => include_bytes!("../assets/icons/ellipsis.svg"),
        Icon::Lock => include_bytes!("../assets/icons/lock.svg"),
        Icon::Shield => include_bytes!("../assets/icons/shield.svg"),
    }
}
