// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! POSIX `command` utility argument handling and command-name reporting.

use fshell_engine::Env;

use super::type_cmd::{CommandResolution, resolve_command};

/// Search path for standard utilities, independent of the caller's PATH.
/// POSIX exposes the host's default ordering through confstr(_CS_PATH).
pub fn default_path() -> &'static str {
    static PATH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        // SAFETY: a null destination with zero length queries the required size.
        let length = unsafe { libc::confstr(libc::_CS_PATH, std::ptr::null_mut(), 0) };
        if length > 1 {
            let mut buffer = vec![0u8; length];
            // SAFETY: buffer is writable for length bytes; confstr includes NUL.
            let written =
                unsafe { libc::confstr(libc::_CS_PATH, buffer.as_mut_ptr().cast(), length) };
            if written > 1 && written <= length {
                return String::from_utf8_lossy(&buffer[..written - 1]).into_owned();
            }
        }
        "/bin:/usr/bin".to_string()
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandAction {
    Noop,
    Execute {
        command_name: String,
        args: Vec<String>,
        use_default_path: bool,
    },
    Lookup {
        names: Vec<String>,
        verbose: bool,
        use_default_path: bool,
    },
    Error(String),
}

/// Parse the options and operands accepted by the POSIX `command` utility.
pub fn parse_command_args(args: &[String]) -> CommandAction {
    let mut index = 0;
    let mut use_default_path = false;
    let mut verbose = None;

    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            index += 1;
            break;
        }
        let Some(flags) = arg.strip_prefix('-') else {
            break;
        };
        if flags.is_empty() {
            break;
        }

        for flag in flags.chars() {
            match flag {
                'p' => use_default_path = true,
                'v' | 'V' => {
                    let requested_verbose = flag == 'V';
                    if verbose.is_some_and(|existing| existing != requested_verbose) {
                        return CommandAction::Error(
                            "-v and -V cannot be used together".to_string(),
                        );
                    }
                    verbose = Some(requested_verbose);
                }
                other => {
                    return CommandAction::Error(format!("illegal option -- {other}"));
                }
            }
        }
        index += 1;
    }

    let operands = &args[index..];
    if let Some(verbose) = verbose {
        return if operands.is_empty() {
            CommandAction::Noop
        } else {
            CommandAction::Lookup {
                names: operands.to_vec(),
                verbose,
                use_default_path,
            }
        };
    }

    let Some(command_name) = operands.first() else {
        return CommandAction::Noop;
    };
    CommandAction::Execute {
        command_name: command_name.clone(),
        args: operands[1..].to_vec(),
        use_default_path,
    }
}

/// Render the `command -v` or `command -V` result for each operand.
pub fn lookup_commands(
    names: &[String],
    verbose: bool,
    env: &Env,
    path_override: Option<&str>,
) -> (i32, String) {
    let mut status = 0;
    let mut output = String::new();

    for name in names {
        let resolution = resolve_command(name, env, path_override);
        let rendered = match resolution {
            CommandResolution::Keyword => {
                if verbose {
                    format!("{name} is a shell keyword")
                } else {
                    name.clone()
                }
            }
            CommandResolution::Alias(expansion) => {
                if verbose {
                    format!("{name} is an alias for {expansion}")
                } else {
                    format!("alias {name}='{}'", quote_alias(&expansion))
                }
            }
            CommandResolution::SpecialBuiltin => {
                if verbose {
                    format!("{name} is a special shell builtin")
                } else {
                    name.clone()
                }
            }
            CommandResolution::Function => {
                if verbose {
                    format!("{name} is a function")
                } else {
                    name.clone()
                }
            }
            CommandResolution::Builtin => {
                if verbose {
                    format!("{name} is a shell builtin")
                } else {
                    name.clone()
                }
            }
            CommandResolution::Path(path) => {
                if verbose {
                    format!("{name} is {path}")
                } else {
                    path
                }
            }
            CommandResolution::NotFound => {
                status = 1;
                continue;
            }
        };
        output.push_str(&rendered);
        output.push('\n');
    }

    (status, output)
}

fn quote_alias(expansion: &str) -> String {
    expansion.replace('\'', "'\\''")
}
