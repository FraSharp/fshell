// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Interactive visual configuration manager (config TUI).

pub mod app;
pub mod schema;
pub mod widgets;

use fshell_engine::Env;

pub fn run_config_tui(env: &Env) -> Result<(), String> {
    app::run(env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_tui::app::{App, Category, Focus, ModalType};
    use crate::config_tui::schema::OptionItem;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use fshell_core::theme::Theme;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    #[test]
    fn test_app_initialization() {
        let env = Env::new();
        let app = App::new(&env);

        assert_eq!(app.categories.len(), 6);
        assert_eq!(app.focus, Focus::Sidebar);
        assert_eq!(app.sidebar_selected, 0);
        assert_eq!(app.content_selected, 0);
        assert!(!app.dirty);
        assert_eq!(app.modal, ModalType::None);
        assert_eq!(app.current_category(), Category::ShellOptions);
    }

    #[test]
    fn test_categories_and_labels() {
        let all = Category::all();
        assert_eq!(all.len(), 6);
        assert_eq!(Category::ShellOptions.label(), "Shell Options");
        assert_eq!(Category::Themes.label(), "Themes & Colors");
        assert_eq!(Category::Aliases.label(), "Aliases");
        assert_eq!(Category::Hooks.label(), "Hooks");
        assert_eq!(Category::EnvVars.label(), "Environment Vars");
        assert_eq!(Category::Prompt.label(), "Prompt & Studio");
    }

    #[test]
    fn test_navigation_and_tab_switching() {
        let env = Env::new();
        let mut app = App::new(&env);

        // Move down in sidebar
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::empty()));
        assert_eq!(app.sidebar_selected, 1);
        assert_eq!(app.current_category(), Category::Themes);

        // Tab switches focus to content
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::empty()));
        assert_eq!(app.focus, Focus::Content);

        // Left switches focus back to sidebar
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::empty()));
        assert_eq!(app.focus, Focus::Sidebar);
    }

    #[test]
    fn test_option_cycling_and_sync() {
        let env = Env::new();
        let mut app = App::new(&env);

        // Initially autocd is true
        assert!(env.options.read().autocd);

        // Focus content and press Space on first option (autocd)
        app.focus = Focus::Content;
        app.content_selected = 0;
        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::empty()));

        assert!(!env.options.read().autocd);
        assert!(app.dirty);

        // Cycle back
        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::empty()));
        assert!(env.options.read().autocd);
    }

    #[test]
    fn test_schema_option_loading() {
        let env = Env::new();
        let options = OptionItem::load_all(&env);
        assert!(options.len() >= 25);
        assert!(options.iter().any(|o| o.key == "confirm_destructive"));
        assert!(options.iter().any(|o| o.key == "pipefail"));
        assert!(options.iter().any(|o| o.key == "theme"));
        assert!(options.iter().any(|o| o.key == "sandbox_mode"));
    }

    #[test]
    fn test_search_and_filtering() {
        let env = Env::new();
        let mut app = App::new(&env);

        // Start search
        app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::empty()));
        assert_eq!(app.focus, Focus::Search);

        // Type "pipe"
        app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::empty()));
        app.handle_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::empty()));
        app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::empty()));
        app.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::empty()));
        assert_eq!(app.search_query, "pipe");

        let filtered = app.filtered_option_indices();
        assert!(!filtered.is_empty());
        for idx in filtered {
            let opt = &app.options[idx];
            assert!(
                opt.key.contains("pipe")
                    || opt.label.to_lowercase().contains("pipe")
                    || opt.description.to_lowercase().contains("pipe")
            );
        }
    }

    #[test]
    fn test_alias_management() {
        let env = Env::new();
        let mut app = App::new(&env);
        app.sidebar_selected = 2; // Aliases

        // Add alias via modal
        app.focus = Focus::Content;
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty()));
        assert!(matches!(app.modal, ModalType::TwoFieldAlias { .. }));

        // Fill modal fields
        if let ModalType::TwoFieldAlias {
            name, expansion, ..
        } = &mut app.modal
        {
            *name = "ll".into();
            *expansion = "ls -la".into();
        }

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert_eq!(app.modal, ModalType::None);
        assert!(app.dirty);

        let aliases = env.scope.aliases.read();
        assert_eq!(aliases.get("ll"), Some(&"ls -la".to_string()));
        drop(aliases);

        // Delete alias
        app.content_selected = 0;
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::empty()));
        let aliases = env.scope.aliases.read();
        assert!(!aliases.contains_key("ll"));
    }

    #[test]
    fn test_hook_management() {
        let env = Env::new();
        let mut app = App::new(&env);
        app.sidebar_selected = 3; // Hooks

        // Add hook
        app.focus = Focus::Content;
        app.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::empty()));
        assert!(matches!(app.modal, ModalType::TwoFieldHook { .. }));

        if let ModalType::TwoFieldHook { event, fn_name, .. } = &mut app.modal {
            *event = "precmd".into();
            *fn_name = "my_precmd_fn".into();
        }

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert_eq!(app.modal, ModalType::None);

        let reg = env.hooks.registry.read();
        assert!(
            reg.get("precmd")
                .unwrap()
                .contains(&"my_precmd_fn".to_string())
        );
        drop(reg);

        // Delete hook
        app.content_selected = 0;
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::empty()));
        let reg = env.hooks.registry.read();
        assert!(
            !reg.get("precmd")
                .unwrap_or(&Vec::new())
                .contains(&"my_precmd_fn".to_string())
        );
    }

    #[test]
    fn test_widget_rendering_all_tabs() {
        let env = Env::new();
        let app = App::new(&env);
        let area = Rect::new(0, 0, 100, 30);
        let mut buf = Buffer::empty(area);

        widgets::sidebar::render(Rect::new(0, 0, 24, 30), &mut buf, &app);
        widgets::shell_options::render(Rect::new(24, 0, 76, 30), &mut buf, &app);
        widgets::theme_browser::render(Rect::new(24, 0, 76, 30), &mut buf, &app);
        widgets::aliases::render(Rect::new(24, 0, 76, 30), &mut buf, &app);
        widgets::hooks::render(Rect::new(24, 0, 76, 30), &mut buf, &app);
        widgets::env_vars::render(Rect::new(24, 0, 76, 30), &mut buf, &app);
        widgets::prompt_config::render(Rect::new(24, 0, 76, 30), &mut buf, &app);

        // Modals
        let theme = Theme::default_theme();
        widgets::modal::render_text_input(
            area,
            &mut buf,
            &theme,
            widgets::modal::TextInputProps {
                title: "Title",
                label: "Label",
                value: "Value",
                details: None,
                error: None,
            },
        );
        widgets::modal::render_two_field_modal(
            area,
            &mut buf,
            &theme,
            widgets::modal::TwoFieldModalProps {
                title: "Title",
                field1_label: "Field 1",
                field1_val: "Val 1",
                field2_label: "Field 2",
                field2_val: "Val 2",
                active_field: 0,
                error: None,
            },
        );
        widgets::modal::render_confirm_dialog(
            area,
            &mut buf,
            &theme,
            "Save?",
            "Do you want to save?",
        );
        widgets::modal::render_help_modal(area, &mut buf, &theme);
    }

    #[test]
    fn test_env_registration_and_dispatch() {
        let env = Env::new();
        assert!(env.get_config_tui_handler().is_none());
        assert!(env.run_config_tui().is_err());

        crate::init(&env);
        assert!(env.get_config_tui_handler().is_some());
    }
}
