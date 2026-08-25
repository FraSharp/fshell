// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! POSIX `printf` builtin implementation.
//!
//! Conforms to POSIX.1-2024 / IEEE Std 1003.1 `printf` utility specification:
//! - Format specifiers: `%s`, `%b`, `%c`, `%d`, `%i`, `%o`, `%u`, `%x`, `%X`, `%f`, `%%`.
//! - Escape sequence handling (`\n`, `\t`, `\a`, `\b`, `\f`, `\r`, `\v`, `\\`, `\0NNN`, `\c`).
//! - Format string recycling (re-using format until all arguments are consumed).

pub fn printf_posix(args: &[String]) -> Result<i32, String> {
    if args.is_empty() {
        return Ok(0);
    }

    let format = &args[0];
    let values = &args[1..];

    let output = format_printf(format, values)?;
    print!("{}", output);
    Ok(0)
}

pub fn format_printf(format: &str, args: &[String]) -> Result<String, String> {
    let mut out = String::new();
    let mut arg_idx = 0;

    // In POSIX, if args are present, format string is repeated until all args are consumed.
    // If no args, format is evaluated once.
    loop {
        let (rendered, advanced) = render_format_pass(format, args, arg_idx)?;
        out.push_str(&rendered);
        arg_idx += advanced;

        // If no arguments were provided or all arguments are consumed, stop
        if args.is_empty() || arg_idx >= args.len() || advanced == 0 {
            break;
        }
    }

    Ok(out)
}

fn render_format_pass(
    format: &str,
    args: &[String],
    start_arg_idx: usize,
) -> Result<(String, usize), String> {
    let mut out = String::new();
    let mut chars = format.chars().peekable();
    let mut arg_idx = start_arg_idx;

    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('a') => out.push('\x07'),
                Some('b') => out.push('\x08'),
                Some('f') => out.push('\x0c'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('v') => out.push('\x0b'),
                Some('\\') => out.push('\\'),
                Some('\'') => out.push('\''),
                Some('"') => out.push('"'),
                Some('0') => {
                    // Octal: \0NNN (up to 3 octal digits)
                    let mut oct_str = String::new();
                    while oct_str.len() < 3
                        && matches!(chars.peek(), Some(&d) if ('0'..='7').contains(&d))
                    {
                        if let Some(d) = chars.next() {
                            oct_str.push(d);
                        }
                    }
                    let byte = u8::from_str_radix(&oct_str, 8).unwrap_or(0);
                    out.push(byte as char);
                }
                Some('c') => {
                    // \c stops all further output
                    return Ok((out, arg_idx - start_arg_idx));
                }
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else if c == '%' {
            if chars.peek() == Some(&'%') {
                chars.next();
                out.push('%');
                continue;
            }

            // Parse specifier: %[-+ #0]*[width]*[.precision]*specifier
            let mut spec_str = String::from("%");
            let mut specifier = None;

            while let Some(&nc) = chars.peek() {
                chars.next();
                spec_str.push(nc);
                if matches!(
                    nc,
                    's' | 'b'
                        | 'c'
                        | 'd'
                        | 'i'
                        | 'o'
                        | 'u'
                        | 'x'
                        | 'X'
                        | 'f'
                        | 'e'
                        | 'E'
                        | 'g'
                        | 'G'
                ) {
                    specifier = Some(nc);
                    break;
                }
                if !(nc.is_ascii_digit() || matches!(nc, '-' | '+' | ' ' | '#' | '0' | '.')) {
                    // Unrecognized specifier char
                    break;
                }
            }

            let next_arg = if arg_idx < args.len() {
                let a = &args[arg_idx];
                arg_idx += 1;
                a.as_str()
            } else {
                ""
            };

            let parsed_spec = parse_spec(&spec_str);

            match specifier {
                Some('s') => {
                    let mut s = next_arg.to_string();
                    if let Some(spec) = &parsed_spec {
                        if let Some(prec) = spec.precision
                            && s.len() > prec
                        {
                            s.truncate(prec);
                        }
                        out.push_str(&apply_formatting(&s, spec, false));
                    } else {
                        out.push_str(&s);
                    }
                }
                Some('b') => {
                    // %b expands backslash escapes in the argument, including \c
                    let mut b_chars = next_arg.chars().peekable();
                    let mut hit_c = false;
                    let mut b_out = String::new();
                    while let Some(bc) = b_chars.next() {
                        if bc == '\\' {
                            match b_chars.next() {
                                Some('a') => b_out.push('\x07'),
                                Some('b') => b_out.push('\x08'),
                                Some('f') => b_out.push('\x0c'),
                                Some('n') => b_out.push('\n'),
                                Some('r') => b_out.push('\r'),
                                Some('t') => b_out.push('\t'),
                                Some('v') => b_out.push('\x0b'),
                                Some('\\') => b_out.push('\\'),
                                Some('0') => {
                                    let mut oct_str = String::new();
                                    while oct_str.len() < 3
                                        && matches!(b_chars.peek(), Some(&d) if ('0'..='7').contains(&d))
                                    {
                                        if let Some(d) = b_chars.next() {
                                            oct_str.push(d);
                                        }
                                    }
                                    let byte = u8::from_str_radix(&oct_str, 8).unwrap_or(0);
                                    b_out.push(byte as char);
                                }
                                Some('c') => {
                                    hit_c = true;
                                    break;
                                }
                                Some(o) => {
                                    b_out.push('\\');
                                    b_out.push(o);
                                }
                                None => b_out.push('\\'),
                            }
                        } else {
                            b_out.push(bc);
                        }
                    }
                    if let Some(spec) = &parsed_spec {
                        out.push_str(&apply_formatting(&b_out, spec, false));
                    } else {
                        out.push_str(&b_out);
                    }
                    if hit_c {
                        return Ok((out, arg_idx - start_arg_idx));
                    }
                }
                Some('c') => {
                    let ch = next_arg.chars().next().unwrap_or('\0').to_string();
                    if let Some(spec) = &parsed_spec {
                        out.push_str(&apply_formatting(&ch, spec, false));
                    } else {
                        out.push_str(&ch);
                    }
                }
                Some('d') | Some('i') => {
                    let num: i64 = parse_printf_int(next_arg);
                    let mut num_str = num.to_string();
                    if let Some(spec) = &parsed_spec {
                        if spec.always_sign && num >= 0 {
                            num_str = format!("+{}", num_str);
                        } else if spec.space_sign && num >= 0 {
                            num_str = format!(" {}", num_str);
                        }
                        out.push_str(&apply_formatting(&num_str, spec, true));
                    } else {
                        out.push_str(&num_str);
                    }
                }
                Some('u') => {
                    let num: u64 = parse_printf_int(next_arg) as u64;
                    let num_str = num.to_string();
                    if let Some(spec) = &parsed_spec {
                        out.push_str(&apply_formatting(&num_str, spec, true));
                    } else {
                        out.push_str(&num_str);
                    }
                }
                Some('o') => {
                    let num: u64 = parse_printf_int(next_arg) as u64;
                    let num_str = format!("{:o}", num);
                    if let Some(spec) = &parsed_spec {
                        out.push_str(&apply_formatting(&num_str, spec, true));
                    } else {
                        out.push_str(&num_str);
                    }
                }
                Some('x') => {
                    let num: u64 = parse_printf_int(next_arg) as u64;
                    let num_str = format!("{:x}", num);
                    if let Some(spec) = &parsed_spec {
                        out.push_str(&apply_formatting(&num_str, spec, true));
                    } else {
                        out.push_str(&num_str);
                    }
                }
                Some('X') => {
                    let num: u64 = parse_printf_int(next_arg) as u64;
                    let num_str = format!("{:X}", num);
                    if let Some(spec) = &parsed_spec {
                        out.push_str(&apply_formatting(&num_str, spec, true));
                    } else {
                        out.push_str(&num_str);
                    }
                }
                Some('f') | Some('e') | Some('E') | Some('g') | Some('G') => {
                    let num: f64 = next_arg.trim().parse::<f64>().unwrap_or(0.0);
                    let prec = parsed_spec.as_ref().and_then(|s| s.precision).unwrap_or(6);
                    let num_str = format!("{:.*}", prec, num);
                    if let Some(spec) = &parsed_spec {
                        out.push_str(&apply_formatting(&num_str, spec, true));
                    } else {
                        out.push_str(&num_str);
                    }
                }
                _ => {
                    out.push_str(&spec_str);
                }
            }
        } else {
            out.push(c);
        }
    }

    Ok((out, arg_idx - start_arg_idx))
}

struct FormatSpec {
    left_align: bool,
    zero_pad: bool,
    always_sign: bool,
    space_sign: bool,
    width: Option<usize>,
    precision: Option<usize>,
    #[allow(dead_code)]
    specifier: char,
}

fn parse_spec(spec_str: &str) -> Option<FormatSpec> {
    if !spec_str.starts_with('%') || spec_str.len() < 2 {
        return None;
    }
    let specifier = spec_str.chars().last()?;
    let middle = &spec_str[1..spec_str.len() - 1];

    let mut left_align = false;
    let mut zero_pad = false;
    let mut always_sign = false;
    let mut space_sign = false;

    let mut chars = middle.chars().peekable();

    while let Some(&c) = chars.peek() {
        match c {
            '-' => {
                left_align = true;
                chars.next();
            }
            '+' => {
                always_sign = true;
                chars.next();
            }
            ' ' => {
                space_sign = true;
                chars.next();
            }
            '0' => {
                zero_pad = true;
                chars.next();
            }
            '#' => {
                chars.next();
            }
            _ => break,
        }
    }

    if left_align {
        zero_pad = false;
    }

    let mut width_str = String::new();
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            width_str.push(c);
            chars.next();
        } else {
            break;
        }
    }
    let width = width_str.parse::<usize>().ok();

    let mut precision = None;
    if chars.peek() == Some(&'.') {
        chars.next();
        let mut prec_str = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_ascii_digit() {
                prec_str.push(c);
                chars.next();
            } else {
                break;
            }
        }
        precision = Some(prec_str.parse::<usize>().unwrap_or(0));
    }

    Some(FormatSpec {
        left_align,
        zero_pad,
        always_sign,
        space_sign,
        width,
        precision,
        specifier,
    })
}

fn apply_formatting(val_str: &str, spec: &FormatSpec, is_number: bool) -> String {
    let mut s = val_str.to_string();
    if let Some(w) = spec.width
        && s.len() < w
    {
        let pad_len = w - s.len();
        if spec.left_align {
            s.push_str(&" ".repeat(pad_len));
        } else if spec.zero_pad && is_number {
            if s.starts_with('-') {
                s = format!("-{}{}", "0".repeat(pad_len), &s[1..]);
            } else if s.starts_with('+') {
                s = format!("+{}{}", "0".repeat(pad_len), &s[1..]);
            } else {
                s = format!("{}{}", "0".repeat(pad_len), s);
            }
        } else {
            s = format!("{}{}", " ".repeat(pad_len), s);
        }
    }
    s
}

fn parse_printf_int(s: &str) -> i64 {
    let t = s.trim();
    if t.is_empty() {
        return 0;
    }
    // Check for character constant: 'a' -> 97
    if t.starts_with('\'') || t.starts_with('"') {
        let mut chars = t.chars().skip(1);
        if let Some(c) = chars.next() {
            return c as i64;
        }
    }
    // Check hex/octal prefixes
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16).unwrap_or(0)
    } else if t.starts_with('0') && t.len() > 1 && t.chars().all(|c| ('0'..='7').contains(&c)) {
        i64::from_str_radix(t, 8).unwrap_or(0)
    } else {
        t.parse::<i64>().unwrap_or(0)
    }
}
