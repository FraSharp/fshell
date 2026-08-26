// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::ShellError;
use fshell_core::Val;
use fshell_engine::{Env, PipeSender, PipeStream, PipelinePayload};
use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;

// ANSI helpers

fn cyan(s: &str, color: bool) -> String {
    if color {
        format!("\x1b[1;36m{}\x1b[0m", s)
    } else {
        s.to_string()
    }
}
fn yellow(s: &str, color: bool) -> String {
    if color {
        format!("\x1b[1;33m{}\x1b[0m", s)
    } else {
        s.to_string()
    }
}
fn green(s: &str, color: bool) -> String {
    if color {
        format!("\x1b[1;32m{}\x1b[0m", s)
    } else {
        s.to_string()
    }
}
fn dim(s: &str, color: bool) -> String {
    if color {
        format!("\x1b[2;37m{}\x1b[0m", s)
    } else {
        s.to_string()
    }
}

// Data types

pub mod topics;
pub mod tui;
pub use topics::{HelpCategory, HelpExample, HelpFlag, HelpTopic, TOPICS};

pub fn help_topics() -> &'static [HelpTopic] {
    TOPICS
}

pub fn find_topic(name: &str) -> Option<&'static HelpTopic> {
    let name_lower = name.trim().to_lowercase();
    // Legacy alias resolution and grouped-topic shortcuts
    let canonical = match name_lower.as_str() {
        "try" | "catch" => "try-catch",
        "caps" | "with" => "with-caps",
        "if" | "else" => "if",
        "pushd" | "popd" | "dirs" => "directory-stack",
        "fs-read" | "fs-write" | "fs-readwrite" => "filesystem-capabilities",
        "net-connect" | "net-all" => "network-capabilities",
        "env-read" | "env-write" => "environment-capabilities",
        "eval_direnv" | "load_env_file" => "direnv",
        "break" | "continue" => "loop-control",
        other => other,
    };
    TOPICS.iter().find(|t| {
        t.name == canonical
            || canonical == t.name.replace('-', "")
            || canonical == t.name.replace('-', " ")
            || canonical == t.name.replace('-', "_")
    })
}

pub fn topics_by_category() -> Vec<(HelpCategory, Vec<&'static HelpTopic>)> {
    let mut map: HashMap<HelpCategory, Vec<&'static HelpTopic>> = HashMap::new();
    for topic in TOPICS {
        map.entry(topic.category).or_default().push(topic);
    }
    let mut pairs: Vec<_> = map.into_iter().collect();
    pairs.sort_by_key(|(cat, _)| *cat);
    pairs
}

// Rendering

/// Horizontal rule used in rendered help output.
const RULE: &str = "────────────────────────────────────────────────";

/// Full detail view: `help <topic>`
pub fn render_full(topic: &HelpTopic, color: bool) -> String {
    let mut buf = String::new();

    // Title bar
    buf.push_str(&green(RULE, color));
    buf.push('\n');
    buf.push_str("  ");
    buf.push_str(&cyan(&format!("{} — {}", topic.name, topic.summary), color));
    buf.push('\n');
    buf.push_str(&green(RULE, color));
    buf.push('\n');

    // Category tag
    buf.push_str("  ");
    buf.push_str(&dim(topic.category.label(), color));
    buf.push('\n');
    buf.push('\n');

    // Syntax
    if !topic.syntax.is_empty() {
        buf.push_str(&yellow("SYNTAX", color));
        buf.push('\n');
        buf.push_str("  ");
        buf.push_str(topic.syntax);
        buf.push_str("\n\n");
    }

    // Description
    buf.push_str(&yellow("DESCRIPTION", color));
    buf.push('\n');
    for line in topic.description.lines() {
        buf.push_str("  ");
        buf.push_str(line);
        buf.push('\n');
    }
    buf.push('\n');

    // Examples
    if !topic.examples.is_empty() {
        buf.push_str(&yellow("EXAMPLES", color));
        buf.push('\n');
        for ex in topic.examples {
            buf.push_str("  ");
            buf.push_str(&cyan(ex.input, color));
            buf.push('\n');
            buf.push_str("    ");
            buf.push_str(&dim(ex.explanation, color));
            buf.push('\n');
        }
        buf.push('\n');
    }

    // Flags
    if !topic.flags.is_empty() {
        buf.push_str(&yellow("FLAGS", color));
        buf.push('\n');
        for f in topic.flags {
            buf.push_str(&format!("  {}    {}\n", yellow(f.flag, color), f.desc));
        }
        buf.push('\n');
    }

    // Related
    if !topic.related.is_empty() {
        buf.push_str(&yellow("RELATED", color));
        buf.push('\n');
        buf.push_str("  ");
        let related: Vec<String> = topic.related.iter().map(|r| cyan(r, color)).collect();
        buf.push_str(&related.join(", "));
        buf.push_str("\n\n");
    }

    buf
}

/// Compact name + summary listing: `help --quick` (or `-q`)
pub fn render_quick_list(color: bool) -> String {
    let mut buf = String::new();
    for topic in TOPICS {
        buf.push_str(&format!(
            "  {:<20} {}\n",
            cyan(topic.name, color),
            dim(topic.summary, color)
        ));
    }
    buf
}

/// Names-only listing: `help --topics` (or `-t`)
pub fn render_names_only(_color: bool) -> String {
    let mut buf = String::new();
    for topic in TOPICS {
        buf.push_str(&format!("{}\n", topic.name));
    }
    buf
}

/// All topics in full: `help --all`
pub fn render_all(color: bool) -> String {
    let mut buf = String::new();
    for topic in TOPICS {
        buf.push_str(&render_full(topic, color));
    }
    buf
}

/// Examples-only for a topic: `help <topic> --examples`
pub fn render_examples(topic: &HelpTopic, color: bool) -> String {
    let mut buf = String::new();
    buf.push_str(&cyan(&format!("{} — {}", topic.name, topic.summary), color));
    buf.push('\n');
    for ex in topic.examples {
        buf.push_str(&format!("  {}\n", cyan(ex.input, color)));
        buf.push_str(&format!("    {}\n", dim(ex.explanation, color)));
    }
    buf
}

/// Category index: `help` with no arguments
pub fn render_category_index(color: bool) -> String {
    let mut buf = String::new();
    let by_cat = topics_by_category();

    for (cat, topics) in &by_cat {
        buf.push_str(&format!(
            "  {:<15} {}",
            cyan(&format!("{} ({})", cat.header(), topics.len()), color),
            cat.description(),
        ));
        buf.push('\n');
    }

    buf.push('\n');
    buf.push_str(&dim(
        "  help <category> for topics · help <topic> for details · -v for full",
        color,
    ));
    buf.push('\n');
    buf
}

/// Filtered topic listing by category: `help --category builtin`
pub fn render_category_filtered(category: HelpCategory, color: bool) -> String {
    let mut buf = String::new();
    buf.push_str(&yellow(category.header(), color));
    buf.push('\n');

    for topic in TOPICS {
        if topic.category == category {
            buf.push_str(&format!(
                "  {:<12} {}\n",
                cyan(topic.name, color),
                topic.summary
            ));
        }
    }
    buf.push('\n');
    buf.push_str(&dim("  help <topic> for details (-v for full)", color));
    buf.push('\n');
    buf
}

/// Compact topic listing for a single category: `help <category>`
pub fn render_category_topics(category: HelpCategory, color: bool) -> String {
    let mut buf = String::new();
    buf.push_str(&yellow(category.header(), color));
    buf.push('\n');

    let by_cat = topics_by_category();
    let topics = by_cat.iter().find(|(c, _)| *c == category).map(|(_, t)| t);

    if let Some(topics) = topics {
        for topic in topics {
            buf.push_str(&format!(
                "  {:<12} {}\n",
                cyan(topic.name, color),
                topic.summary
            ));
        }
        buf.push('\n');
        buf.push_str(&dim(
            &format!(
                "help <topic> for details (-v for full) · {} total",
                topics.len()
            ),
            color,
        ));
        buf.push('\n');
    }

    buf
}

pub fn category_from_str(s: &str) -> Option<HelpCategory> {
    match s.to_lowercase().as_str() {
        "builtins" | "builtin" => Some(HelpCategory::Builtin),
        "pipeline" | "pipelines" => Some(HelpCategory::Pipeline),
        "language" | "language-constructs" => Some(HelpCategory::Language),
        "security" => Some(HelpCategory::Security),
        "concepts" | "shell-concepts" => Some(HelpCategory::Concepts),
        _ => None,
    }
}

/// Compact card view: `help <topic>` (without -v)
pub fn render_compact(topic: &HelpTopic, color: bool) -> String {
    let mut buf = String::new();

    // Title line
    buf.push_str(&format!(
        "{} — {}\n",
        cyan(topic.name, color),
        topic.summary
    ));

    // Category tag
    buf.push_str(&format!("  {}\n\n", dim(topic.category.label(), color)));

    // Syntax
    if !topic.syntax.is_empty() {
        buf.push_str(&format!("  {}: {}\n", yellow("Usage", color), topic.syntax));
        buf.push('\n');
    }

    // Short description (first 3 lines max)
    let desc: Vec<&str> = topic.description.lines().collect();
    let desc_short: Vec<&&str> = desc.iter().take(3).collect();
    for line in &desc_short {
        buf.push_str(&format!("  {}\n", line));
    }
    buf.push('\n');

    // Flags (up to 5, with overflow hint)
    if !topic.flags.is_empty() {
        let shown = topic.flags.iter().take(5).collect::<Vec<_>>();
        for f in &shown {
            buf.push_str(&format!("  {}    {}\n", yellow(f.flag, color), f.desc));
        }
        if topic.flags.len() > 5 {
            buf.push_str(&format!(
                "  {}  … and {} more\n",
                dim("...", color),
                topic.flags.len() - 5
            ));
        }
        buf.push('\n');
    }

    // Examples (at most 2)
    let examples_shown = topic.examples.iter().take(2);
    for ex in examples_shown {
        buf.push_str(&format!("  {}\n", cyan(ex.input, color)));
        buf.push_str(&format!("    {}\n", dim(ex.explanation, color)));
    }
    buf.push('\n');

    // Pointer to full detail
    buf.push_str(&dim(
        &format!(
            "help {} -v for full detail · help {} --examples for more",
            topic.name, topic.name,
        ),
        color,
    ));
    buf.push('\n');
    buf
}

/// Search across all topics: `help --search <text>`
/// Returns (formatted_text, found_any)
pub fn render_search_results(text: &str, color: bool) -> (String, bool) {
    let text_lower = text.to_lowercase();
    let mut results: Vec<(&HelpTopic, &str)> = Vec::new();

    for topic in TOPICS {
        if topic.name.to_lowercase().contains(&text_lower) {
            results.push((topic, "name"));
        } else if topic.summary.to_lowercase().contains(&text_lower) {
            results.push((topic, "summary"));
        } else if topic.description.to_lowercase().contains(&text_lower) {
            results.push((topic, "description"));
        }
    }

    if results.is_empty() {
        return (
            format!(
                "No results for '{}'. Try 'help -t' to browse all topics.\n",
                text
            ),
            false,
        );
    }

    let mut buf = format!("Results for \"{}\":\n", text);
    for (topic, match_type) in &results {
        let tag = match *match_type {
            "name" => cyan("(exact)", color),
            "summary" => dim("(summary)", color),
            "description" => dim("(description)", color),
            _ => unreachable!(),
        };
        buf.push_str(&format!(
            "  {:<12} {}  {}\n",
            cyan(topic.name, color),
            topic.summary,
            tag,
        ));
    }
    buf.push('\n');
    buf.push_str(&dim("help <topic> for details", color));
    buf.push('\n');
    (buf, true)
}

// Terminal detection & pager

fn terminal_height() -> u16 {
    if let Ok((_, h)) = crossterm::terminal::size() {
        h
    } else {
        24
    }
}

fn needs_pager(text: &str, in_rx: &Option<PipeStream>, env: &Env) -> bool {
    if fshell_engine::is_test_mode()
        || !fshell_engine::is_interactive_terminal()
        || !fshell_engine::is_stdout_a_tty()
        || in_rx.is_some()
        || env.is_captured
        || !env.is_last_stage
    {
        return false;
    }
    let line_count = text.lines().count() + 1;
    let height = terminal_height() as usize;
    line_count > height.saturating_sub(3)
}

fn pipe_to_pager(text: &str) {
    let pager = std::env::var("PAGER").unwrap_or_else(|_| "less".to_string());
    let args: &[&str] = if pager == "less" { &["-R"] } else { &[] };

    if let Ok(mut child) = std::process::Command::new(&pager)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
            drop(stdin);
        }
        let _ = child.wait();
    } else {
        // fallback: just print
        println!("{}", text);
    }
}

// Structured flag parser for long-term maintainability

struct HelpFlags {
    all: bool,
    quick: bool,
    topics: bool,
    verbose: bool,
    examples: bool,
    tui: bool,
    search: Option<String>,
    structured: bool,
    category: Option<HelpCategory>,
    topic: Option<String>,
}

fn parse_help_flags(str_args: &[String]) -> HelpFlags {
    let mut flags = HelpFlags {
        all: false,
        quick: false,
        topics: false,
        verbose: false,
        examples: false,
        tui: false,
        search: None,
        structured: false,
        category: None,
        topic: None,
    };

    let mut i = 0;
    while i < str_args.len() {
        let a = &str_args[i];
        match a.as_str() {
            "--all" | "-a" => flags.all = true,
            "--quick" | "-q" => flags.quick = true,
            "--topics" | "-t" => flags.topics = true,
            "--verbose" | "-v" => flags.verbose = true,
            "--examples" | "-e" => flags.examples = true,
            "--tui" | "-i" => flags.tui = true,
            "--structured" | "--json" => flags.structured = true,
            "--search" | "-s" => {
                if let Some(val) = str_args.get(i + 1)
                    && !val.starts_with('-')
                {
                    flags.search = Some(val.clone());
                    i += 1;
                }
            }
            "--category" | "-c" => {
                if let Some(val) = str_args.get(i + 1)
                    && !val.starts_with('-')
                {
                    flags.category = category_from_str(val);
                    i += 1;
                }
            }
            _ => {
                if let Some(val) = a.strip_prefix("--search=") {
                    flags.search = Some(val.to_string());
                } else if let Some(val) = a.strip_prefix("--category=") {
                    flags.category = category_from_str(val);
                } else if !a.starts_with('-') && flags.topic.is_none() {
                    flags.topic = Some(a.clone());
                }
            }
        }
        i += 1;
    }

    flags
}

/// Return a filtered list of topics matching a category or search.
fn filter_topics_by(
    category: Option<HelpCategory>,
    search_text: Option<&str>,
) -> Vec<&'static HelpTopic> {
    TOPICS
        .iter()
        .filter(|t| {
            let cat_ok = category.is_none_or(|c| t.category == c);
            let search_ok = search_text.is_none_or(|q| {
                let q = q.to_lowercase();
                t.name.to_lowercase().contains(&q)
                    || t.summary.to_lowercase().contains(&q)
                    || t.description.to_lowercase().contains(&q)
            });
            cat_ok && search_ok
        })
        .collect()
}

// Help builtin handler

pub fn help_builtin(
    _in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
) -> Result<(), ShellError> {
    let color = env.options.read().error_color;

    // Parse flags from the argument list
    let str_args: Vec<String> = args
        .iter()
        .filter_map(|v| match v {
            Val::String(s) => Some(s.clone()),
            _ => None,
        })
        .collect();

    let flags = parse_help_flags(&str_args);

    let is_structured = flags.structured || env.is_captured || !env.is_last_stage;

    let run_tui_mode = !fshell_engine::is_test_mode()
        && (flags.tui
            || (flags.topic.is_none()
                && flags.search.is_none()
                && !flags.all
                && !flags.quick
                && !flags.topics
                && flags.category.is_none()
                && fshell_engine::is_stdout_a_tty()
                && _in_rx.is_none()
                && !is_structured));

    if run_tui_mode {
        return tui::run_tui(env);
    }

    if is_structured {
        let val = if let Some(ref text) = flags.search {
            let list: Vec<Val> = filter_topics_by(flags.category, Some(text))
                .iter()
                .map(|t| t.to_val())
                .collect();
            Val::List(list)
        } else if let Some(ref name) = flags.topic {
            if let Some(topic) = find_topic(name) {
                topic.to_val()
            } else if let Some(category) = category_from_str(name) {
                let list: Vec<Val> = TOPICS
                    .iter()
                    .filter(|t| t.category == category)
                    .map(|t| t.to_val())
                    .collect();
                Val::List(list)
            } else {
                // search fallback
                let list: Vec<Val> = filter_topics_by(flags.category, Some(name))
                    .iter()
                    .map(|t| t.to_val())
                    .collect();
                Val::List(list)
            }
        } else {
            // No topic or search query — optionally filtered by category
            let list: Vec<Val> = filter_topics_by(flags.category, None)
                .iter()
                .map(|t| t.to_val())
                .collect();
            Val::List(list)
        };

        tokio::spawn(async move {
            let _ = tx.send(PipelinePayload::Data(Arc::new(val))).await;
        });
        return Ok(());
    }

    let help_text = if flags.all {
        render_all(color)
    } else if flags.topics {
        // --topics / -t: names only
        if let Some(category) = flags.category {
            let names: Vec<&str> = TOPICS
                .iter()
                .filter(|t| t.category == category)
                .map(|t| t.name)
                .collect();
            names.join("\n") + "\n"
        } else {
            render_names_only(color)
        }
    } else if flags.quick {
        // --quick / -q: name + summary
        if let Some(category) = flags.category {
            let mut buf = String::new();
            for t in TOPICS {
                if t.category == category {
                    buf.push_str(&format!(
                        "  {:<20} {}\n",
                        cyan(t.name, color),
                        dim(t.summary, color)
                    ));
                }
            }
            buf
        } else {
            render_quick_list(color)
        }
    } else if let Some(ref search_text) = flags.search {
        render_search_results(search_text, color).0
    } else if let Some(ref name) = flags.topic {
        if flags.examples {
            match find_topic(name) {
                Some(topic) => render_examples(topic, color),
                None => format!(
                    "No help entry for '{}'. Try 'help' for available topics.\n",
                    name
                ),
            }
        } else if flags.verbose {
            match find_topic(name) {
                Some(topic) => render_full(topic, color),
                None => format!(
                    "No help entry for '{}'. Try 'help' for available topics.\n",
                    name
                ),
            }
        } else {
            // Try topic first, then category, then search fallback
            match find_topic(name) {
                Some(topic) => render_compact(topic, color),
                None => match category_from_str(name) {
                    Some(category) => render_category_topics(category, color),
                    None => {
                        // Fallback: search and suggest
                        let (result, found) = render_search_results(name, color);
                        if found {
                            format!("No topic named '{}'.\n{}\n", name, result)
                        } else {
                            format!(
                                "No help entry for '{}'. Try 'help' for available topics.\n",
                                name
                            )
                        }
                    }
                },
            }
        }
    } else if let Some(category) = flags.category {
        // --category without a topic name
        render_category_filtered(category, color)
    } else {
        render_category_index(color)
    };

    // Decide on pager vs direct output
    if needs_pager(&help_text, &_in_rx, env) {
        pipe_to_pager(&help_text);
        drop(tx);
    } else {
        let text = help_text;
        tokio::spawn(async move {
            let _ = tx
                .send(PipelinePayload::Data(Arc::new(Val::String(text))))
                .await;
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_help_rendering_no_color() {
        let topic = &TOPICS[0];
        let rendered = render_compact(topic, false);
        assert!(
            !rendered.contains("\x1b["),
            "Rendered compact output should not contain ANSI escape codes: {:?}",
            rendered
        );
    }

    #[test]
    fn test_tui_fuzzy_matching() {
        let matches = tui::get_matching_topics("filt");
        let names: Vec<&str> = matches.iter().map(|t| t.name).collect();
        assert!(names.contains(&"filter"), "Should contain 'filter'");
        assert!(names.contains(&"head"), "Should contain 'head'");
        assert!(names.contains(&"tail"), "Should contain 'tail'");
        assert_eq!(names[0], "filter");
    }

    #[test]
    fn test_parse_help_flags_all() {
        let args = vec!["--all".to_string()];
        let flags = parse_help_flags(&args);
        assert!(flags.all);
        assert!(!flags.quick);
    }

    #[test]
    fn test_parse_help_flags_short() {
        let args = vec!["-q".to_string()];
        let flags = parse_help_flags(&args);
        assert!(!flags.all);
        assert!(flags.quick);
    }

    #[test]
    fn test_parse_help_flags_topics_with_t() {
        let args = vec!["-t".to_string()];
        let flags = parse_help_flags(&args);
        assert!(flags.topics);
    }

    #[test]
    fn test_parse_help_flags_category() {
        let args = vec!["--category".to_string(), "builtin".to_string()];
        let flags = parse_help_flags(&args);
        assert_eq!(flags.category, Some(HelpCategory::Builtin));
    }

    #[test]
    fn test_parse_help_flags_category_eq() {
        let args = vec!["--category=pipeline".to_string()];
        let flags = parse_help_flags(&args);
        assert_eq!(flags.category, Some(HelpCategory::Pipeline));
    }

    #[test]
    fn test_parse_help_flags_tui_with_i() {
        let args = vec!["-i".to_string()];
        let flags = parse_help_flags(&args);
        assert!(flags.tui);
    }

    #[test]
    fn test_parse_help_flags_search() {
        let args = vec!["-s".to_string(), "filter".to_string()];
        let flags = parse_help_flags(&args);
        assert_eq!(flags.search, Some("filter".to_string()));
    }

    #[test]
    fn test_render_names_only() {
        let rendered = render_names_only(false);
        assert!(rendered.contains("ls\n"));
        assert!(rendered.contains("filter\n"));
        assert!(!rendered.contains(" — "));
    }

    #[test]
    fn test_render_compact_shows_flags() {
        let ls = find_topic("ls").unwrap();
        let rendered = render_compact(ls, false);
        assert!(rendered.contains("-a"));
        assert!(rendered.contains("-v"));
    }

    #[test]
    fn test_render_category_filtered() {
        let rendered = render_category_filtered(HelpCategory::Pipeline, false);
        assert!(rendered.contains("filter"));
        assert!(rendered.contains("map"));
        assert!(!rendered.contains("BUILTINS"));
    }

    #[test]
    fn test_filter_topics_by() {
        let results = filter_topics_by(Some(HelpCategory::Security), None);
        assert!(!results.is_empty());
        assert!(results.iter().all(|t| t.category == HelpCategory::Security));
    }

    #[test]
    fn test_needs_pager_in_test_mode() {
        let env = Env::new();
        let long_text = "line\n".repeat(100);
        assert!(!needs_pager(&long_text, &None, &env));
    }
}
