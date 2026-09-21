//! GPUI-free command catalogue shared by key bindings, menus, the palette and Settings.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommandId {
    PreviousMessage,
    NextMessage,
    ConfirmSelection,
    Compose,
    Reply,
    ReplyAll,
    Forward,
    Send,
    Trash,
    Archive,
    ArchiveConversation,
    ToggleRead,
    MoveToMailbox,
    ToggleThreadGrouping,
    ToggleRemoteImages,
    ToggleBodyPreference,
    FocusSearch,
    OpenPalette,
    PalettePrevious,
    PaletteNext,
    PaletteConfirm,
    SaveEvent,
    Cancel,
    CycleAppearance,
    ToggleDiagnostics,
    ShowMail,
    ShowCalendar,
    ShowSettings,
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CommandContext {
    Global,
    List,
    Reading,
    Compose,
    Calendar,
    Search,
    Editor,
}

impl CommandContext {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Global => "Global",
            Self::List => "Message list",
            Self::Reading => "Reading",
            Self::Compose => "Compose",
            Self::Calendar => "Calendar",
            Self::Search => "Search",
            Self::Editor => "Editor",
        }
    }

    pub const fn key_context(self) -> &'static str {
        match self {
            Self::Global => "Global",
            Self::List => "List",
            Self::Reading => "Reading",
            Self::Compose => "Compose",
            Self::Calendar => "Calendar",
            Self::Search => "Search",
            Self::Editor => "Editor",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InputGuard {
    Allow,
    Block,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EnableGuard {
    Always,
    HasMessages,
    HasSelection,
    HasReading,
    ComposeOpen,
    EventEditorOpen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MenuGroup {
    Application,
    File,
    Edit,
    Message,
    Calendar,
    View,
}

impl MenuGroup {
    pub const fn title(self) -> &'static str {
        match self {
            Self::Application => "Snail",
            Self::File => "File",
            Self::Edit => "Edit",
            Self::Message => "Message",
            Self::Calendar => "Calendar",
            Self::View => "View",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MenuPlacement {
    pub group: MenuGroup,
    pub order: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Command {
    pub id: CommandId,
    pub name: &'static str,
    pub title: &'static str,
    pub default_binding: &'static str,
    pub menu: Option<MenuPlacement>,
    pub context: CommandContext,
    pub input: InputGuard,
    pub enable: EnableGuard,
}

const fn menu(group: MenuGroup, order: u16) -> Option<MenuPlacement> {
    Some(MenuPlacement { group, order })
}

macro_rules! command {
    ($id:ident, $name:literal, $title:literal, $binding:literal, $menu:expr, $context:ident, $input:ident, $enable:ident) => {
        Command {
            id: CommandId::$id,
            name: $name,
            title: $title,
            default_binding: $binding,
            menu: $menu,
            context: CommandContext::$context,
            input: InputGuard::$input,
            enable: EnableGuard::$enable,
        }
    };
}

pub const COMMANDS: &[Command] = &[
    command!(
        PreviousMessage,
        "list.up",
        "Previous message",
        "up",
        None,
        List,
        Block,
        HasMessages
    ),
    command!(
        NextMessage,
        "list.down",
        "Next message",
        "down",
        None,
        List,
        Block,
        HasMessages
    ),
    command!(
        ConfirmSelection,
        "list.confirm",
        "Confirm selection",
        "enter",
        None,
        List,
        Block,
        Always
    ),
    command!(
        Compose,
        "mail.compose",
        "New message",
        "c",
        menu(MenuGroup::File, 10),
        Global,
        Block,
        Always
    ),
    command!(
        Reply,
        "mail.reply",
        "Reply",
        "cmd-r",
        menu(MenuGroup::Message, 10),
        Reading,
        Block,
        HasReading
    ),
    command!(
        ReplyAll,
        "mail.reply_all",
        "Reply all",
        "cmd-shift-r",
        menu(MenuGroup::Message, 20),
        Reading,
        Block,
        HasReading
    ),
    command!(
        Forward,
        "mail.forward",
        "Forward",
        "f",
        menu(MenuGroup::Message, 30),
        Reading,
        Block,
        HasReading
    ),
    command!(
        Send,
        "mail.send",
        "Send message",
        "cmd-shift-d",
        menu(MenuGroup::Message, 40),
        Compose,
        Allow,
        ComposeOpen
    ),
    command!(
        Trash,
        "mail.trash",
        "Move to Trash",
        "cmd-backspace",
        menu(MenuGroup::Message, 60),
        List,
        Block,
        HasSelection
    ),
    command!(
        Archive,
        "mail.archive",
        "Archive",
        "cmd-shift-a",
        menu(MenuGroup::Message, 50),
        List,
        Block,
        HasSelection
    ),
    command!(
        ArchiveConversation,
        "mail.archive_thread",
        "Archive conversation",
        "e",
        menu(MenuGroup::Message, 55),
        Reading,
        Block,
        HasReading
    ),
    command!(
        ToggleRead,
        "mail.toggle_read",
        "Toggle read",
        "u",
        menu(MenuGroup::Message, 70),
        List,
        Block,
        HasSelection
    ),
    command!(
        MoveToMailbox,
        "mail.move",
        "Move to mailbox",
        "m",
        menu(MenuGroup::Message, 80),
        List,
        Block,
        HasSelection
    ),
    command!(
        ToggleThreadGrouping,
        "mail.group_threads",
        "Toggle thread grouping",
        "g",
        menu(MenuGroup::View, 40),
        List,
        Block,
        HasMessages
    ),
    command!(
        ToggleRemoteImages,
        "reader.remote_images",
        "Toggle remote images",
        "f4",
        menu(MenuGroup::View, 50),
        Reading,
        Block,
        HasReading
    ),
    command!(
        ToggleBodyPreference,
        "reader.body_preference",
        "Toggle HTML / plain text",
        "f5",
        menu(MenuGroup::View, 60),
        Reading,
        Block,
        HasReading
    ),
    command!(
        FocusSearch,
        "search.focus",
        "Search mail",
        "cmd-f",
        menu(MenuGroup::Edit, 10),
        Global,
        Allow,
        Always
    ),
    command!(
        OpenPalette,
        "palette.open",
        "Command or date search",
        "cmd-k",
        menu(MenuGroup::Edit, 20),
        Global,
        Allow,
        Always
    ),
    command!(
        PalettePrevious,
        "palette.up",
        "Previous palette result",
        "up",
        None,
        Search,
        Allow,
        Always
    ),
    command!(
        PaletteNext,
        "palette.down",
        "Next palette result",
        "down",
        None,
        Search,
        Allow,
        Always
    ),
    command!(
        PaletteConfirm,
        "palette.confirm",
        "Open palette result",
        "enter",
        None,
        Search,
        Allow,
        Always
    ),
    command!(
        SaveEvent,
        "calendar.save",
        "Save event",
        "cmd-enter",
        menu(MenuGroup::Calendar, 10),
        Editor,
        Allow,
        EventEditorOpen
    ),
    command!(
        Cancel,
        "dialog.cancel",
        "Cancel or close",
        "escape",
        None,
        Global,
        Allow,
        Always
    ),
    command!(
        CycleAppearance,
        "view.theme",
        "Cycle appearance",
        "f3",
        menu(MenuGroup::View, 70),
        Global,
        Allow,
        Always
    ),
    command!(
        ToggleDiagnostics,
        "view.diagnostics",
        "Toggle diagnostics",
        "f2",
        menu(MenuGroup::View, 80),
        Global,
        Allow,
        Always
    ),
    command!(
        ShowMail,
        "workspace.mail",
        "Show Mail",
        "cmd-1",
        menu(MenuGroup::View, 10),
        Global,
        Allow,
        Always
    ),
    command!(
        ShowCalendar,
        "workspace.calendar",
        "Show Calendar",
        "cmd-2",
        menu(MenuGroup::View, 20),
        Global,
        Allow,
        Always
    ),
    command!(
        ShowSettings,
        "workspace.settings",
        "Show Settings",
        "cmd-comma",
        menu(MenuGroup::Application, 10),
        Global,
        Allow,
        Always
    ),
    command!(
        Quit,
        "app.quit",
        "Quit Snail",
        "cmd-q",
        menu(MenuGroup::Application, 100),
        Global,
        Allow,
        Always
    ),
];

pub fn commands() -> &'static [Command] {
    COMMANDS
}

pub fn command(id: CommandId) -> &'static Command {
    COMMANDS
        .iter()
        .find(|command| command.id == id)
        .expect("every CommandId belongs to the catalogue")
}

pub fn context_predicate(command: &Command) -> String {
    let base = command.context.key_context();
    match command.input {
        InputGuard::Allow => base.to_string(),
        InputGuard::Block => format!("{base} && !Input"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutPlatform {
    Mac,
    Other,
}

pub fn display_binding(binding: &str, platform: ShortcutPlatform) -> String {
    match platform {
        ShortcutPlatform::Mac => binding
            .replace("cmd-", "⌘")
            .replace("shift-", "⇧")
            .replace("backspace", "⌫")
            .replace("enter", "↵")
            .replace("escape", "Esc")
            .replace("up", "↑")
            .replace("down", "↓"),
        ShortcutPlatform::Other => binding
            .replace("cmd-", "Ctrl+")
            .replace("shift-", "Shift+")
            .replace('-', "+"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn catalogue_metadata_is_complete_and_unique() {
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        for command in commands() {
            assert!(ids.insert(command.id), "duplicate command id");
            assert!(
                names.insert(command.name),
                "duplicate command name {}",
                command.name
            );
            assert!(!command.title.is_empty());
            assert!(!command.default_binding.is_empty());
            assert!(!context_predicate(command).is_empty());
        }
    }

    #[test]
    fn handoff_shortcuts_are_exact() {
        let binding = |id| command(id).default_binding;
        assert_eq!(binding(CommandId::PreviousMessage), "up");
        assert_eq!(binding(CommandId::NextMessage), "down");
        assert_eq!(binding(CommandId::Reply), "cmd-r");
        assert_eq!(binding(CommandId::Send), "cmd-shift-d");
        assert_eq!(binding(CommandId::Trash), "cmd-backspace");
        assert_eq!(binding(CommandId::Archive), "cmd-shift-a");
        assert_eq!(binding(CommandId::FocusSearch), "cmd-f");
        assert_eq!(binding(CommandId::OpenPalette), "cmd-k");
        assert_eq!(binding(CommandId::SaveEvent), "cmd-enter");
        assert_eq!(binding(CommandId::Cancel), "escape");
    }

    #[test]
    fn printable_shortcuts_are_blocked_inside_inputs() {
        for id in [
            CommandId::Compose,
            CommandId::Forward,
            CommandId::ArchiveConversation,
            CommandId::ToggleRead,
            CommandId::MoveToMailbox,
            CommandId::ToggleThreadGrouping,
        ] {
            assert!(context_predicate(command(id)).ends_with("&& !Input"));
        }
        assert_eq!(context_predicate(command(CommandId::OpenPalette)), "Global");
    }

    #[test]
    fn bindings_render_with_platform_glyphs() {
        assert_eq!(display_binding("cmd-shift-a", ShortcutPlatform::Mac), "⌘⇧a");
        assert_eq!(
            display_binding("cmd-backspace", ShortcutPlatform::Mac),
            "⌘⌫"
        );
        assert_eq!(
            display_binding("cmd-enter", ShortcutPlatform::Other),
            "Ctrl+enter"
        );
    }
}
