// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! POSIX `printf` builtin implementation.
//!
//! Conforms to POSIX.1-2024 / IEEE Std 1003.1 `printf` utility specification:
//! - Format specifiers: `%s`, `%b`, `%c`, `%d`, `%i`, `%o`, `%u`, `%x`, `%X`, `%f`, `%%`.
//! - Escape sequence handling (`\n`, `\t`, `\a`, `\b`, `\f`, `\r`, `\v`, `\\`, `\0NNN`, `\c`).
//! - Format string recycling (re-using format until all arguments are consumed).

#[derive(Debug)]
pub struct PrintfResult {
    pub output: String,
    pub status: i32,
    pub diagnostics: Vec<String>,
}

pub fn printf_posix(args: &[String]) -> Result<i32, String> {
    if args.is_empty() {
        return Ok(0);
    }

    let format = &args[0];
    let values = &args[1..];

    let result = format_printf_with_status(format, values)?;
    for diagnostic in &result.diagnostics {
        eprintln!("{diagnostic}");
    }
    print!("{}", result.output);
    Ok(result.status)
}

pub fn format_printf(format: &str, args: &[String]) -> Result<String, String> {
    Ok(format_printf_with_status(format, args)?.output)
}

pub fn format_printf_with_status(format: &str, args: &[String]) -> Result<PrintfResult, String> {
    let mut out = String::new();
    let mut arg_idx = 0;
    let mut diagnostics = Vec::new();

    // In POSIX, if args are present, format string is repeated until all args are consumed.
    // If no args, format is evaluated once.
    loop {
        let pass = render_format_pass(format, args, arg_idx)?;
        out.push_str(&pass.output);
        arg_idx += pass.advanced;
        diagnostics.extend(pass.diagnostics);

        // If no arguments were provided or all arguments are consumed, stop
        if args.is_empty() || arg_idx >= args.len() || pass.advanced == 0 {
            break;
        }
    }

    Ok(PrintfResult {
        output: out,
        status: if diagnostics.is_empty() { 0 } else { 1 },
        diagnostics,
    })
}

struct FormatPass {
    output: String,
    advanced: usize,
    diagnostics: Vec<String>,
}

fn render_format_pass(
    format: &str,
    args: &[String],
    start_arg_idx: usize,
) -> Result<FormatPass, String> {
    let mut out = String::new();
    let mut diagnostics = Vec::new();
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
                    return Ok(FormatPass {
                        output: out,
                        advanced: arg_idx - start_arg_idx,
                        diagnostics,
                    });
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
                            truncate_on_char_boundary(&mut s, prec);
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
                        return Ok(FormatPass {
                            output: out,
                            advanced: arg_idx - start_arg_idx,
                            diagnostics,
                        });
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
                    let num = match parse_printf_int(next_arg) {
                        Ok(num) => num,
                        Err(error) => {
                            diagnostics.push(error);
                            0
                        }
                    };
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
                    let num: u64 = match parse_printf_int(next_arg) {
                        Ok(num) => num as u64,
                        Err(error) => {
                            diagnostics.push(error);
                            0
                        }
                    };
                    let num_str = num.to_string();
                    if let Some(spec) = &parsed_spec {
                        out.push_str(&apply_formatting(&num_str, spec, true));
                    } else {
                        out.push_str(&num_str);
                    }
                }
                Some('o') => {
                    let num: u64 = match parse_printf_int(next_arg) {
                        Ok(num) => num as u64,
                        Err(error) => {
                            diagnostics.push(error);
                            0
                        }
                    };
                    let num_str = format!("{:o}", num);
                    if let Some(spec) = &parsed_spec {
                        out.push_str(&apply_formatting(&num_str, spec, true));
                    } else {
                        out.push_str(&num_str);
                    }
                }
                Some('x') => {
                    let num: u64 = match parse_printf_int(next_arg) {
                        Ok(num) => num as u64,
                        Err(error) => {
                            diagnostics.push(error);
                            0
                        }
                    };
                    let num_str = format!("{:x}", num);
                    if let Some(spec) = &parsed_spec {
                        out.push_str(&apply_formatting(&num_str, spec, true));
                    } else {
                        out.push_str(&num_str);
                    }
                }
                Some('X') => {
                    let num: u64 = match parse_printf_int(next_arg) {
                        Ok(num) => num as u64,
                        Err(error) => {
                            diagnostics.push(error);
                            0
                        }
                    };
                    let num_str = format!("{:X}", num);
                    if let Some(spec) = &parsed_spec {
                        out.push_str(&apply_formatting(&num_str, spec, true));
                    } else {
                        out.push_str(&num_str);
                    }
                }
                Some('f') | Some('e') | Some('E') | Some('g') | Some('G') => {
                    let num = match next_arg.trim().parse::<f64>() {
                        Ok(num) => num,
                        Err(error) => {
                            diagnostics.push(format!(
                                "printf: {:?}: invalid number ({})",
                                next_arg.trim(),
                                error
                            ));
                            0.0
                        }
                    };
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

    Ok(FormatPass {
        output: out,
        advanced: arg_idx - start_arg_idx,
        diagnostics,
    })
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

/// Shrinks `s` to at most `max_bytes` bytes without splitting a UTF-8
/// character. The precision in `%.Ns` bounds the bytes emitted, but a partial
/// multi-byte character is never produced; back off to the previous boundary.
fn truncate_on_char_boundary(s: &mut String, max_bytes: usize) {
    if max_bytes >= s.len() {
        return;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
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

fn parse_printf_int(s: &str) -> Result<i64, String> {
    let t = s.trim();
    if t.is_empty() {
        return Ok(0);
    }
    // Check for character constant: 'a' -> 97
    if t.starts_with('\'') || t.starts_with('"') {
        let mut chars = t.chars().skip(1);
        if let Some(c) = chars.next() {
            return Ok(c as i64);
        }
        return Err(format!("printf: {:?}: invalid number", t));
    }
    // Check hex/octal prefixes
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16)
            .map_err(|error| format!("printf: {:?}: invalid number ({})", t, error))
    } else if t.starts_with('0') && t.len() > 1 && t.chars().all(|c| ('0'..='7').contains(&c)) {
        i64::from_str_radix(t, 8)
            .map_err(|error| format!("printf: {:?}: invalid number ({})", t, error))
    } else {
        t.parse::<i64>()
            .map_err(|error| format!("printf: {:?}: invalid number ({})", t, error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(format: &str, args: &[&str]) -> String {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        format_printf(format, &owned).expect("format_printf")
    }

    #[test]
    fn string_precision_cuts_on_char_boundary() {
        // A byte precision inside a multi-byte character must not panic; it
        // backs off to the previous boundary rather than truncating blindly.
        assert_eq!(fmt("%.1s", &["é"]), "");
        assert_eq!(fmt("%.2s", &["é"]), "é");
        assert_eq!(fmt("%.3s", &["日本語"]), "日");
        assert_eq!(fmt("%.99s", &["é"]), "é");
        assert_eq!(fmt("%.2s", &["hello"]), "he");
    }
}
