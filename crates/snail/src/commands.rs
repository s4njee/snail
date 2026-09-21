//! GPUI projection of the framework-free command catalogue.

use gpui::Action;
use gpui_kit::{App, KeyBinding, Menu, MenuItem};
use snail_ui::commands::{
    Command, CommandContext, CommandId, InputGuard, MenuGroup, commands, context_predicate,
};

/// Every shortcut, native menu item and palette command dispatches this one action type.
#[derive(Action, Clone, Copy, Debug, PartialEq, Eq)]
#[action(namespace = snail, no_json)]
pub struct RunCommand(pub CommandId);

fn binding_predicates(command: &Command) -> Vec<String> {
    let mut predicates = vec![context_predicate(command)];
    if command.input == InputGuard::Allow {
        predicates.push(format!("{} && Input", command.context.key_context()));
        // A Global command is available inside every nested region. When an Input owns the
        // focused path, project the same action at that nearer context as well so its built-in
        // Escape/arrows/Enter bindings cannot shadow an explicitly input-safe app command.
        if command.context == CommandContext::Global {
            predicates.extend(
                [
                    CommandContext::List,
                    CommandContext::Reading,
                    CommandContext::Compose,
                    CommandContext::Calendar,
                    CommandContext::Search,
                    CommandContext::Editor,
                ]
                .map(|context| format!("{} && Input", context.key_context())),
            );
        }
    }
    predicates
}

pub fn key_bindings() -> Vec<KeyBinding> {
    let mut bindings = Vec::new();
    for command in commands() {
        for predicate in binding_predicates(command) {
            bindings.push(KeyBinding::new(
                command.default_binding,
                RunCommand(command.id),
                Some(&predicate),
            ));
        }
    }
    bindings
}

pub fn native_menus() -> Vec<Menu> {
    const GROUPS: [MenuGroup; 6] = [
        MenuGroup::Application,
        MenuGroup::File,
        MenuGroup::Edit,
        MenuGroup::Message,
        MenuGroup::Calendar,
        MenuGroup::View,
    ];
    GROUPS
        .into_iter()
        .filter_map(|group| {
            let mut entries: Vec<_> = commands()
                .iter()
                .filter_map(|command| {
                    command
                        .menu
                        .filter(|placement| placement.group == group)
                        .map(|placement| (placement.order, command))
                })
                .collect();
            entries.sort_by_key(|(order, _)| *order);
            (!entries.is_empty()).then(|| {
                Menu::new(group.title()).items(
                    entries.into_iter().map(|(_, command)| {
                        MenuItem::action(command.title, RunCommand(command.id))
                    }),
                )
            })
        })
        .collect()
}

pub fn install(cx: &mut App) {
    cx.bind_keys(key_bindings());
    #[cfg(target_os = "macos")]
    cx.set_menus(native_menus());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalogue_binding_constructs_in_gpui() {
        let bindings = key_bindings();
        assert_eq!(
            bindings.len(),
            commands()
                .iter()
                .map(binding_predicates)
                .map(|p| p.len())
                .sum::<usize>()
        );
        assert!(
            bindings
                .iter()
                .all(|binding| !binding.keystrokes().is_empty())
        );
        let action_name = bindings[0].action().name();
        assert!(
            bindings
                .iter()
                .all(|binding| binding.action().name() == action_name)
        );
    }

    #[test]
    fn global_input_safe_commands_project_into_nested_input_contexts() {
        let cancel = commands()
            .iter()
            .find(|command| command.id == CommandId::Cancel)
            .unwrap();
        let predicates = binding_predicates(cancel);
        assert!(predicates.iter().any(|value| value == "Search && Input"));
        assert!(predicates.iter().any(|value| value == "Compose && Input"));
        assert!(predicates.iter().any(|value| value == "Editor && Input"));
    }

    #[test]
    fn every_menu_placement_projects_to_a_native_item() {
        let expected = commands()
            .iter()
            .filter(|command| command.menu.is_some())
            .count();
        let actual = native_menus()
            .iter()
            .map(|menu| menu.items.len())
            .sum::<usize>();
        assert_eq!(actual, expected);
    }
}
