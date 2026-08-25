// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::prompt_config::*;

pub fn by_name(name: &str) -> Option<PromptConfig> {
    match name {
        "minimal" => Some(minimal()),
        "powerline" => Some(powerline()),
        "nerd" => Some(nerd()),
        "classic" => Some(classic()),
        _ => None,
    }
}

pub fn available() -> &'static [&'static str] {
    &["minimal", "powerline", "nerd", "classic"]
}

fn segment_config(
    r#type: SegmentType,
    fg: Option<&str>,
    bg: Option<&str>,
    bold: bool,
) -> SegmentConfig {
    let parse_spec = |s: &str| {
        if s.starts_with('#') {
            ColorSpec::Hex(s.into())
        } else {
            ColorSpec::Named(s.into())
        }
    };
    SegmentConfig {
        bg: bg.map(parse_spec),
        ..SegmentConfig::new(r#type, fg.map(parse_spec), bold)
    }
}

fn minimal() -> PromptConfig {
    PromptConfig {
        separator_style: SeparatorStyle::None,
        preset: Some("minimal".into()),
        left: vec![
            segment_config(SegmentType::User, Some("keyword"), None, true),
            segment_config(SegmentType::Pwd, Some("string"), None, true),
            segment_config(SegmentType::GitBranch, Some("ok"), None, true),
            {
                let mut s = segment_config(SegmentType::GitStatus, Some("ok"), None, false);
                s.hide_when_clean = true;
                s.show_only_in_repo = true;
                s
            },
            {
                let mut s = segment_config(SegmentType::Char, None, None, true);
                s.fg = Some(ColorSpec::Conditional {
                    ok: "ok".into(),
                    err: "error".into(),
                });
                s
            },
        ],
        right: vec![
            {
                let mut s = segment_config(SegmentType::ExitCode, Some("error"), None, true);
                s.hide_on_zero = true;
                s
            },
            {
                let mut s = segment_config(SegmentType::Duration, Some("muted"), None, false);
                s.hide_under_ms = 1000;
                s
            },
            {
                let mut s = segment_config(SegmentType::Jobs, Some("builtin"), None, false);
                s.hide_on_zero = true;
                s
            },
        ],
        ..Default::default()
    }
}

fn powerline() -> PromptConfig {
    PromptConfig {
        separator_style: SeparatorStyle::Arrow,
        preset: Some("powerline".into()),
        left: vec![
            segment_config(SegmentType::User, Some("white"), Some("keyword"), true),
            segment_config(SegmentType::Pwd, Some("white"), Some("string"), true),
            {
                let mut s = segment_config(SegmentType::GitBranch, Some("white"), Some("ok"), true);
                s.show_only_in_repo = true;
                s
            },
            {
                let mut s = segment_config(SegmentType::Char, None, None, true);
                s.fg = Some(ColorSpec::Named("white".into()));
                s.bg = Some(ColorSpec::Conditional {
                    ok: "ok".into(),
                    err: "error".into(),
                });
                s
            },
        ],
        right: vec![
            {
                let mut s =
                    segment_config(SegmentType::ExitCode, Some("white"), Some("error"), true);
                s.hide_on_zero = true;
                s
            },
            {
                let mut s =
                    segment_config(SegmentType::Duration, Some("white"), Some("muted"), false);
                s.hide_under_ms = 1000;
                s
            },
        ],
        ..Default::default()
    }
}

fn nerd() -> PromptConfig {
    PromptConfig {
        separator_style: SeparatorStyle::Chevron,
        preset: Some("nerd".into()),
        left: vec![
            {
                let mut s = segment_config(SegmentType::Text, Some("keyword"), None, true);
                s.text = Some("".into());
                s.prefix = " ".into();
                s.suffix = " ".into();
                s
            },
            segment_config(SegmentType::User, Some("keyword"), None, true),
            {
                let mut s = segment_config(SegmentType::Text, Some("string"), None, true);
                s.text = Some("".into());
                s.prefix = " ".into();
                s.suffix = " ".into();
                s
            },
            segment_config(SegmentType::Pwd, Some("string"), None, true),
            {
                let mut s = segment_config(SegmentType::Text, Some("ok"), None, true);
                s.text = Some("".into());
                s.prefix = " ".into();
                s.suffix = " ".into();
                s.show_only_in_repo = true;
                s
            },
            {
                let mut s = segment_config(SegmentType::GitBranch, Some("ok"), None, true);
                s.show_only_in_repo = true;
                s
            },
            {
                let mut s = segment_config(SegmentType::Char, None, None, true);
                s.fg = Some(ColorSpec::Conditional {
                    ok: "ok".into(),
                    err: "error".into(),
                });
                s
            },
        ],
        right: vec![
            {
                let mut s = segment_config(SegmentType::ExitCode, Some("error"), None, true);
                s.hide_on_zero = true;
                s
            },
            {
                let mut s = segment_config(SegmentType::Duration, Some("muted"), None, false);
                s.hide_under_ms = 1000;
                s
            },
        ],
        ..Default::default()
    }
}

fn classic() -> PromptConfig {
    PromptConfig {
        separator_style: SeparatorStyle::None,
        preset: Some("classic".into()),
        left: vec![
            {
                let mut s = segment_config(SegmentType::Text, Some("white"), None, false);
                s.text = Some("[".into());
                s
            },
            segment_config(SegmentType::User, Some("keyword"), None, false),
            {
                let mut s = segment_config(SegmentType::Text, Some("white"), None, false);
                s.text = Some("@".into());
                s
            },
            segment_config(SegmentType::Host, Some("builtin"), None, false),
            {
                let mut s = segment_config(SegmentType::Text, Some("white"), None, false);
                s.text = Some(" ".into());
                s
            },
            {
                let mut s = segment_config(SegmentType::Pwd, Some("string"), None, false);
                s.shorten = true;
                s
            },
            {
                let mut s = segment_config(SegmentType::Text, Some("white"), None, false);
                s.text = Some("]".into());
                s
            },
            segment_config(SegmentType::Char, None, None, false),
        ],
        right: vec![],
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_presets_load() {
        for name in available() {
            let cfg = by_name(name)
                .unwrap_or_else(|| panic!("available() returned a name that by_name() can't find"));
            assert_eq!(cfg.preset.as_deref(), Some(*name));
            assert!(!cfg.left.is_empty());
        }
    }

    #[test]
    fn test_unknown_preset_returns_none() {
        assert!(by_name("nonexistent").is_none());
    }

    #[test]
    fn test_powerline_has_backgrounds() {
        let cfg = by_name("powerline")
            .unwrap_or_else(|| panic!("powerline preset should always be available"));
        assert!(cfg.left.iter().any(|s| s.bg.is_some()));
        assert_eq!(cfg.separator_style, SeparatorStyle::Arrow);
    }

    #[test]
    fn test_separator_style_glyphs() {
        assert_eq!(SeparatorStyle::Arrow.glyph(), "\u{e0b0}");
        assert_eq!(SeparatorStyle::Chevron.glyph(), "\u{e0b1}");
        assert_eq!(SeparatorStyle::Pipe.glyph(), "│");
        assert_eq!(SeparatorStyle::None.glyph(), " ");
        assert_eq!(SeparatorStyle::Custom(">>".into()).glyph(), ">>");
    }
}
