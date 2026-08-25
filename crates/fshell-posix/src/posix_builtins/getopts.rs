// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! POSIX `getopts` builtin implementation.
//!
//! Conforms to IEEE Std 1003.1 `getopts` specification:
//! - `getopts optstring name [arg ...]`
//! - Manages `OPTIND` and `OPTARG` in `env.vars`.
//! - Supports required arguments (`x:`) and silent error handling (leading `:` in optstring).

use fshell_core::Val;
use fshell_engine::Env;

pub fn getopts_posix(args: &[String], env: &Env) -> Result<i32, String> {
    if args.len() < 2 {
        return Err("getopts: usage: getopts optstring name [arg ...]".to_string());
    }

    let optstring = &args[0];
    let var_name = &args[1];

    // Arguments to parse: if args.len() > 2, use args[2..], else use positional $@ from env
    let target_args: Vec<String> = if args.len() > 2 {
        args[2..].to_vec()
    } else {
        {
            let vars = env.vars.read();
            if let Some(Val::List(items)) = vars.get("@") {
                items.iter().map(|v| v.to_text()).collect()
            } else {
                Vec::new()
            }
        }
    };

    // Read current OPTIND (1-based index)
    let optind: usize = Some(env.vars.read())
        .and_then(|vars| {
            vars.get("OPTIND")
                .and_then(|v| v.to_text().trim().parse::<usize>().ok())
        })
        .unwrap_or(1);

    // If optind > target_args.len(), no more options
    if optind == 0 || optind > target_args.len() {
        {
            let mut vars = env.vars.write();
            vars.insert(var_name.clone(), Val::String("?".to_string()));
        }
        return Ok(1);
    }

    let current_arg = &target_args[optind - 1];

    // Check if end of options: not starting with '-', exactly "-", or "--"
    if !current_arg.starts_with('-') || current_arg == "-" {
        {
            let mut vars = env.vars.write();
            vars.insert(var_name.clone(), Val::String("?".to_string()));
        }
        return Ok(1);
    }

    if current_arg == "--" {
        // Advance past -- and terminate option parsing
        {
            let mut vars = env.vars.write();
            vars.insert(var_name.clone(), Val::String("?".to_string()));
            vars.insert("OPTIND".to_string(), Val::Int((optind + 1) as i64));
        }
        return Ok(1);
    }

    // Process option flag character
    let silent_mode = optstring.starts_with(':');
    let effective_optstring = if silent_mode {
        &optstring[1..]
    } else {
        optstring.as_str()
    };

    let opt_char = current_arg.chars().nth(1).unwrap_or('?');

    // Look for opt_char in optstring
    if let Some(pos) = effective_optstring.find(opt_char) {
        let requires_arg = effective_optstring[pos..].chars().nth(1) == Some(':');

        if requires_arg {
            // Option argument can be in remainder of current_arg (e.g. -fvalue) or in next argument (e.g. -f value)
            if current_arg.len() > 2 {
                let optarg_val = current_arg[2..].to_string();
                {
                    let mut vars = env.vars.write();
                    vars.insert(var_name.clone(), Val::String(opt_char.to_string()));
                    vars.insert("OPTARG".to_string(), Val::String(optarg_val));
                    vars.insert("OPTIND".to_string(), Val::Int((optind + 1) as i64));
                }
                Ok(0)
            } else if optind < target_args.len() {
                let optarg_val = target_args[optind].clone();
                {
                    let mut vars = env.vars.write();
                    vars.insert(var_name.clone(), Val::String(opt_char.to_string()));
                    vars.insert("OPTARG".to_string(), Val::String(optarg_val));
                    vars.insert("OPTIND".to_string(), Val::Int((optind + 2) as i64));
                }
                Ok(0)
            } else {
                // Missing required argument
                if silent_mode {
                    {
                        let mut vars = env.vars.write();
                        vars.insert(var_name.clone(), Val::String(":".to_string()));
                        vars.insert("OPTARG".to_string(), Val::String(opt_char.to_string()));
                        vars.insert("OPTIND".to_string(), Val::Int((optind + 1) as i64));
                    }
                } else {
                    eprintln!("getopts: option requires an argument -- {}", opt_char);
                    {
                        let mut vars = env.vars.write();
                        vars.insert(var_name.clone(), Val::String("?".to_string()));
                        vars.remove("OPTARG");
                        vars.insert("OPTIND".to_string(), Val::Int((optind + 1) as i64));
                    }
                }
                Ok(0)
            }
        } else {
            // No argument required for this flag
            {
                let mut vars = env.vars.write();
                vars.insert(var_name.clone(), Val::String(opt_char.to_string()));
                vars.remove("OPTARG");
                vars.insert("OPTIND".to_string(), Val::Int((optind + 1) as i64));
            }
            Ok(0)
        }
    } else {
        // Unknown option
        if silent_mode {
            {
                let mut vars = env.vars.write();
                vars.insert(var_name.clone(), Val::String("?".to_string()));
                vars.insert("OPTARG".to_string(), Val::String(opt_char.to_string()));
                vars.insert("OPTIND".to_string(), Val::Int((optind + 1) as i64));
            }
        } else {
            eprintln!("getopts: illegal option -- {}", opt_char);
            {
                let mut vars = env.vars.write();
                vars.insert(var_name.clone(), Val::String("?".to_string()));
                vars.remove("OPTARG");
                vars.insert("OPTIND".to_string(), Val::Int((optind + 1) as i64));
            }
        }
        Ok(0)
    }
}
