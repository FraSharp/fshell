// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::Val;
use fshell_engine::Env;

/// Evaluate a POSIX shell arithmetic expression against an Env.
/// Supports integers, variables from `env.vars`, standard C operators with correct
/// precedence, ternary `? :`, comma operator, and assignment operators.
pub fn eval_arithmetic_expr(expr: &str, env: &Env) -> Result<i64, String> {
    let tokens = tokenize(expr)?;
    let mut parser = ArithParser {
        tokens,
        pos: 0,
        env,
    };
    let result = parser.parse_comma()?;
    if parser.pos < parser.tokens.len() {
        return Err(format!(
            "Unexpected token at end of arithmetic expression: {:?}",
            parser.tokens[parser.pos]
        ));
    }
    Ok(result)
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(i64),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Power,            // **
    Ampersand,        // &
    Pipe,             // |
    Caret,            // ^
    Tilde,            // ~
    Bang,             // !
    ShiftLeft,        // <<
    ShiftRight,       // >>
    Less,             // <
    LessEqual,        // <=
    Greater,          // >
    GreaterEqual,     // >=
    EqualEqual,       // ==
    NotEqual,         // !=
    LogicalAnd,       // &&
    LogicalOr,        // ||
    Question,         // ?
    Colon,            // :
    Assign,           // =
    PlusAssign,       // +=
    MinusAssign,      // -=
    StarAssign,       // *=
    SlashAssign,      // /=
    PercentAssign,    // %=
    ShiftLeftAssign,  // <<=
    ShiftRightAssign, // >>=
    AmpAssign,        // &=
    CaretAssign,      // ^=
    PipeAssign,       // |=
    Increment,        // ++
    Decrement,        // --
    LParen,           // (
    RParen,           // )
    Comma,            // ,
}

fn tokenize(s: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }

        // Numbers: decimal, hex (0x...), octal (0...)
        if c.is_ascii_digit() {
            let start = i;
            if c == '0' && i + 1 < chars.len() && (chars[i + 1] == 'x' || chars[i + 1] == 'X') {
                i += 2;
                while i < chars.len() && chars[i].is_ascii_hexdigit() {
                    i += 1;
                }
                let hex_str: String = chars[start + 2..i].iter().collect();
                let val = i64::from_str_radix(&hex_str, 16)
                    .map_err(|e| format!("Invalid hex number: {e}"))?;
                tokens.push(Token::Number(val));
                continue;
            } else {
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '#') {
                    i += 1;
                }
                let num_str: String = chars[start..i].iter().collect();
                if let Some((base_s, val_s)) = num_str.split_once('#') {
                    let base = base_s
                        .parse::<u32>()
                        .map_err(|e| format!("Invalid base: {e}"))?;
                    if !(2..=36).contains(&base) {
                        return Err(format!("Base must be between 2 and 36, got {base}"));
                    }
                    let val = i64::from_str_radix(val_s, base)
                        .map_err(|e| format!("Invalid number in base {base}: {e}"))?;
                    tokens.push(Token::Number(val));
                    continue;
                }
                let val = if num_str.starts_with('0')
                    && num_str.len() > 1
                    && num_str.chars().all(|c| ('0'..='7').contains(&c))
                {
                    i64::from_str_radix(&num_str, 8).unwrap_or(0)
                } else {
                    num_str
                        .parse::<i64>()
                        .map_err(|e| format!("Invalid integer {num_str}: {e}"))?
                };
                tokens.push(Token::Number(val));
                continue;
            }
        }

        // Identifiers / variable names (including $var or ${var})
        if c.is_alphabetic() || c == '_' || c == '$' {
            let start = i;
            if c == '$' {
                i += 1;
                if i < chars.len() && chars[i] == '{' {
                    i += 1;
                    let id_start = i;
                    while i < chars.len() && chars[i] != '}' {
                        i += 1;
                    }
                    let ident: String = chars[id_start..i].iter().collect();
                    if i < chars.len() && chars[i] == '}' {
                        i += 1;
                    }
                    tokens.push(Token::Ident(ident));
                    continue;
                }
            }
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let mut ident: String = chars[start..i].iter().collect();
            if ident.starts_with('$') {
                ident.remove(0);
            }
            tokens.push(Token::Ident(ident));
            continue;
        }

        // Multi-character operators
        let rem = &chars[i..];
        if rem.starts_with(&['<', '<', '=']) {
            tokens.push(Token::ShiftLeftAssign);
            i += 3;
        } else if rem.starts_with(&['>', '>', '=']) {
            tokens.push(Token::ShiftRightAssign);
            i += 3;
        } else if rem.starts_with(&['*', '*']) {
            tokens.push(Token::Power);
            i += 2;
        } else if rem.starts_with(&['+', '+']) {
            tokens.push(Token::Increment);
            i += 2;
        } else if rem.starts_with(&['-', '-']) {
            tokens.push(Token::Decrement);
            i += 2;
        } else if rem.starts_with(&['+', '=']) {
            tokens.push(Token::PlusAssign);
            i += 2;
        } else if rem.starts_with(&['-', '=']) {
            tokens.push(Token::MinusAssign);
            i += 2;
        } else if rem.starts_with(&['*', '=']) {
            tokens.push(Token::StarAssign);
            i += 2;
        } else if rem.starts_with(&['/', '=']) {
            tokens.push(Token::SlashAssign);
            i += 2;
        } else if rem.starts_with(&['%', '=']) {
            tokens.push(Token::PercentAssign);
            i += 2;
        } else if rem.starts_with(&['&', '=']) {
            tokens.push(Token::AmpAssign);
            i += 2;
        } else if rem.starts_with(&['^', '=']) {
            tokens.push(Token::CaretAssign);
            i += 2;
        } else if rem.starts_with(&['|', '=']) {
            tokens.push(Token::PipeAssign);
            i += 2;
        } else if rem.starts_with(&['<', '<']) {
            tokens.push(Token::ShiftLeft);
            i += 2;
        } else if rem.starts_with(&['>', '>']) {
            tokens.push(Token::ShiftRight);
            i += 2;
        } else if rem.starts_with(&['<', '=']) {
            tokens.push(Token::LessEqual);
            i += 2;
        } else if rem.starts_with(&['>', '=']) {
            tokens.push(Token::GreaterEqual);
            i += 2;
        } else if rem.starts_with(&['=', '=']) {
            tokens.push(Token::EqualEqual);
            i += 2;
        } else if rem.starts_with(&['!', '=']) {
            tokens.push(Token::NotEqual);
            i += 2;
        } else if rem.starts_with(&['&', '&']) {
            tokens.push(Token::LogicalAnd);
            i += 2;
        } else if rem.starts_with(&['|', '|']) {
            tokens.push(Token::LogicalOr);
            i += 2;
        } else {
            // Single character tokens
            match c {
                '+' => tokens.push(Token::Plus),
                '-' => tokens.push(Token::Minus),
                '*' => tokens.push(Token::Star),
                '/' => tokens.push(Token::Slash),
                '%' => tokens.push(Token::Percent),
                '&' => tokens.push(Token::Ampersand),
                '|' => tokens.push(Token::Pipe),
                '^' => tokens.push(Token::Caret),
                '~' => tokens.push(Token::Tilde),
                '!' => tokens.push(Token::Bang),
                '<' => tokens.push(Token::Less),
                '>' => tokens.push(Token::Greater),
                '=' => tokens.push(Token::Assign),
                '?' => tokens.push(Token::Question),
                ':' => tokens.push(Token::Colon),
                '(' => tokens.push(Token::LParen),
                ')' => tokens.push(Token::RParen),
                ',' => tokens.push(Token::Comma),
                _ => {
                    return Err(format!(
                        "Unexpected character in arithmetic expression: '{c}'"
                    ));
                }
            }
            i += 1;
        }
    }

    Ok(tokens)
}

struct ArithParser<'a> {
    tokens: Vec<Token>,
    pos: usize,
    env: &'a Env,
}

impl<'a> ArithParser<'a> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next_tok(&mut self) -> Option<Token> {
        if self.pos < self.tokens.len() {
            let tok = self.tokens[self.pos].clone();
            self.pos += 1;
            Some(tok)
        } else {
            None
        }
    }

    fn resolve_var(&self, name: &str) -> i64 {
        let v_opt = if let Some(ref locals) = self.env.local_vars
            && let Some(v) = locals.read().get(name)
        {
            Some(v.clone())
        } else {
            self.env.vars.read().get(name).cloned()
        };
        if let Some(v) = v_opt {
            let text = match v {
                Val::Int(i) => return i,
                Val::Float(f) => return f as i64,
                other => other.to_text(),
            };
            if let Ok(n) = text.trim().parse::<i64>() {
                return n;
            }
            // Try recursive arithmetic evaluation if string looks like an expression
            if !text.is_empty()
                && let Ok(n) = eval_arithmetic_expr(&text, self.env)
            {
                return n;
            }
        }
        0
    }

    fn set_var(&self, name: &str, val: i64) {
        {
            let mut vars = self.env.vars.write();
            vars.insert(name.to_string(), Val::Int(val));
            if let Some(Val::Map(map)) = vars.get_mut("env") {
                map.insert(ustr::ustr(name), Val::String(val.to_string()));
            }
        }
    }

    // Comma operator: expr, expr, ...
    fn parse_comma(&mut self) -> Result<i64, String> {
        let mut val = self.parse_assignment()?;
        while self.peek() == Some(&Token::Comma) {
            self.next_tok();
            val = self.parse_assignment()?;
        }
        Ok(val)
    }

    // Assignment: ident = expr, ident += expr, ...
    fn parse_assignment(&mut self) -> Result<i64, String> {
        if let Some(Token::Ident(name)) = self.peek().cloned()
            && let Some(op_tok) = self.tokens.get(self.pos + 1).cloned()
        {
            let is_assign = matches!(
                op_tok,
                Token::Assign
                    | Token::PlusAssign
                    | Token::MinusAssign
                    | Token::StarAssign
                    | Token::SlashAssign
                    | Token::PercentAssign
                    | Token::ShiftLeftAssign
                    | Token::ShiftRightAssign
                    | Token::AmpAssign
                    | Token::CaretAssign
                    | Token::PipeAssign
            );
            if is_assign {
                self.pos += 2; // consume ident and operator
                let rhs = self.parse_assignment()?;
                let current = self.resolve_var(&name);
                let new_val = match op_tok {
                    Token::Assign => rhs,
                    Token::PlusAssign => current.wrapping_add(rhs),
                    Token::MinusAssign => current.wrapping_sub(rhs),
                    Token::StarAssign => current.wrapping_mul(rhs),
                    Token::SlashAssign => {
                        if rhs == 0 {
                            return Err("Division by zero in arithmetic expression".to_string());
                        }
                        current.wrapping_div(rhs)
                    }
                    Token::PercentAssign => {
                        if rhs == 0 {
                            return Err("Modulo by zero in arithmetic expression".to_string());
                        }
                        current.wrapping_rem(rhs)
                    }
                    Token::ShiftLeftAssign => current << (rhs.max(0) as u32),
                    Token::ShiftRightAssign => current >> (rhs.max(0) as u32),
                    Token::AmpAssign => current & rhs,
                    Token::CaretAssign => current ^ rhs,
                    Token::PipeAssign => current | rhs,
                    _ => rhs,
                };
                self.set_var(&name, new_val);
                return Ok(new_val);
            }
        }
        self.parse_conditional()
    }

    // Conditional: cond ? true_val : false_val
    fn parse_conditional(&mut self) -> Result<i64, String> {
        let cond = self.parse_logical_or()?;
        if self.peek() == Some(&Token::Question) {
            self.next_tok();
            let true_val = self.parse_comma()?;
            if self.peek() != Some(&Token::Colon) {
                return Err("Expected ':' in ternary conditional".to_string());
            }
            self.next_tok();
            let false_val = self.parse_conditional()?;
            Ok(if cond != 0 { true_val } else { false_val })
        } else {
            Ok(cond)
        }
    }

    // Logical OR: ||
    fn parse_logical_or(&mut self) -> Result<i64, String> {
        let mut left = self.parse_logical_and()?;
        while self.peek() == Some(&Token::LogicalOr) {
            self.next_tok();
            let right = self.parse_logical_and()?;
            left = if left != 0 || right != 0 { 1 } else { 0 };
        }
        Ok(left)
    }

    // Logical AND: &&
    fn parse_logical_and(&mut self) -> Result<i64, String> {
        let mut left = self.parse_bitwise_or()?;
        while self.peek() == Some(&Token::LogicalAnd) {
            self.next_tok();
            let right = self.parse_bitwise_or()?;
            left = if left != 0 && right != 0 { 1 } else { 0 };
        }
        Ok(left)
    }

    // Bitwise OR: |
    fn parse_bitwise_or(&mut self) -> Result<i64, String> {
        let mut left = self.parse_bitwise_xor()?;
        while self.peek() == Some(&Token::Pipe) {
            self.next_tok();
            let right = self.parse_bitwise_xor()?;
            left |= right;
        }
        Ok(left)
    }

    // Bitwise XOR: ^
    fn parse_bitwise_xor(&mut self) -> Result<i64, String> {
        let mut left = self.parse_bitwise_and()?;
        while self.peek() == Some(&Token::Caret) {
            self.next_tok();
            let right = self.parse_bitwise_and()?;
            left ^= right;
        }
        Ok(left)
    }

    // Bitwise AND: &
    fn parse_bitwise_and(&mut self) -> Result<i64, String> {
        let mut left = self.parse_equality()?;
        while self.peek() == Some(&Token::Ampersand) {
            self.next_tok();
            let right = self.parse_equality()?;
            left &= right;
        }
        Ok(left)
    }

    // Equality: ==, !=
    fn parse_equality(&mut self) -> Result<i64, String> {
        let mut left = self.parse_relational()?;
        while let Some(tok) = self.peek().cloned() {
            match tok {
                Token::EqualEqual => {
                    self.next_tok();
                    let right = self.parse_relational()?;
                    left = if left == right { 1 } else { 0 };
                }
                Token::NotEqual => {
                    self.next_tok();
                    let right = self.parse_relational()?;
                    left = if left != right { 1 } else { 0 };
                }
                _ => break,
            }
        }
        Ok(left)
    }

    // Relational: <, <=, >, >=
    fn parse_relational(&mut self) -> Result<i64, String> {
        let mut left = self.parse_shift()?;
        while let Some(tok) = self.peek().cloned() {
            match tok {
                Token::Less => {
                    self.next_tok();
                    let right = self.parse_shift()?;
                    left = if left < right { 1 } else { 0 };
                }
                Token::LessEqual => {
                    self.next_tok();
                    let right = self.parse_shift()?;
                    left = if left <= right { 1 } else { 0 };
                }
                Token::Greater => {
                    self.next_tok();
                    let right = self.parse_shift()?;
                    left = if left > right { 1 } else { 0 };
                }
                Token::GreaterEqual => {
                    self.next_tok();
                    let right = self.parse_shift()?;
                    left = if left >= right { 1 } else { 0 };
                }
                _ => break,
            }
        }
        Ok(left)
    }

    // Bitwise shift: <<, >>
    fn parse_shift(&mut self) -> Result<i64, String> {
        let mut left = self.parse_additive()?;
        while let Some(tok) = self.peek().cloned() {
            match tok {
                Token::ShiftLeft => {
                    self.next_tok();
                    let right = self.parse_additive()?;
                    left = left.wrapping_shl(right.max(0) as u32);
                }
                Token::ShiftRight => {
                    self.next_tok();
                    let right = self.parse_additive()?;
                    left = left.wrapping_shr(right.max(0) as u32);
                }
                _ => break,
            }
        }
        Ok(left)
    }

    // Additive: +, -
    fn parse_additive(&mut self) -> Result<i64, String> {
        let mut left = self.parse_multiplicative()?;
        while let Some(tok) = self.peek().cloned() {
            match tok {
                Token::Plus => {
                    self.next_tok();
                    let right = self.parse_multiplicative()?;
                    left = left.wrapping_add(right);
                }
                Token::Minus => {
                    self.next_tok();
                    let right = self.parse_multiplicative()?;
                    left = left.wrapping_sub(right);
                }
                _ => break,
            }
        }
        Ok(left)
    }

    // Multiplicative: *, /, %
    fn parse_multiplicative(&mut self) -> Result<i64, String> {
        let mut left = self.parse_power()?;
        while let Some(tok) = self.peek().cloned() {
            match tok {
                Token::Star => {
                    self.next_tok();
                    let right = self.parse_power()?;
                    left = left.wrapping_mul(right);
                }
                Token::Slash => {
                    self.next_tok();
                    let right = self.parse_power()?;
                    if right == 0 {
                        return Err("Division by zero in arithmetic expression".to_string());
                    }
                    left = left.wrapping_div(right);
                }
                Token::Percent => {
                    self.next_tok();
                    let right = self.parse_power()?;
                    if right == 0 {
                        return Err("Modulo by zero in arithmetic expression".to_string());
                    }
                    left = left.wrapping_rem(right);
                }
                _ => break,
            }
        }
        Ok(left)
    }

    // Power: ** (right-associative)
    fn parse_power(&mut self) -> Result<i64, String> {
        let left = self.parse_unary()?;
        if self.peek() == Some(&Token::Power) {
            self.next_tok();
            let right = self.parse_power()?;
            if right < 0 {
                Ok(0)
            } else {
                Ok(left.wrapping_pow(right as u32))
            }
        } else {
            Ok(left)
        }
    }

    // Unary: +, -, ~, !, ++x, --x
    fn parse_unary(&mut self) -> Result<i64, String> {
        match self.peek().cloned() {
            Some(Token::Plus) => {
                self.next_tok();
                self.parse_unary()
            }
            Some(Token::Minus) => {
                self.next_tok();
                let v = self.parse_unary()?;
                Ok(v.wrapping_neg())
            }
            Some(Token::Tilde) => {
                self.next_tok();
                let v = self.parse_unary()?;
                Ok(!v)
            }
            Some(Token::Bang) => {
                self.next_tok();
                let v = self.parse_unary()?;
                Ok(if v == 0 { 1 } else { 0 })
            }
            Some(Token::Increment) => {
                self.next_tok();
                if let Some(Token::Ident(name)) = self.next_tok() {
                    let new_val = self.resolve_var(&name).wrapping_add(1);
                    self.set_var(&name, new_val);
                    Ok(new_val)
                } else {
                    Err("Expected variable after prefix '++'".to_string())
                }
            }
            Some(Token::Decrement) => {
                self.next_tok();
                if let Some(Token::Ident(name)) = self.next_tok() {
                    let new_val = self.resolve_var(&name).wrapping_sub(1);
                    self.set_var(&name, new_val);
                    Ok(new_val)
                } else {
                    Err("Expected variable after prefix '--'".to_string())
                }
            }
            _ => self.parse_postfix(),
        }
    }

    // Postfix: x++, x--
    fn parse_postfix(&mut self) -> Result<i64, String> {
        let primary = self.parse_primary()?;
        if let Some(tok) = self.peek().cloned() {
            match tok {
                Token::Increment => {
                    self.next_tok();
                    // If the previous was an ident, increment the var
                    if self.pos >= 2
                        && let Token::Ident(name) = &self.tokens[self.pos - 2]
                    {
                        let new_val = primary.wrapping_add(1);
                        self.set_var(name, new_val);
                    }
                    return Ok(primary);
                }
                Token::Decrement => {
                    self.next_tok();
                    if self.pos >= 2
                        && let Token::Ident(name) = &self.tokens[self.pos - 2]
                    {
                        let new_val = primary.wrapping_sub(1);
                        self.set_var(name, new_val);
                    }
                    return Ok(primary);
                }
                _ => {}
            }
        }
        Ok(primary)
    }

    // Primary: Number, Ident, ( expr )
    fn parse_primary(&mut self) -> Result<i64, String> {
        match self.next_tok() {
            Some(Token::Number(n)) => Ok(n),
            Some(Token::Ident(name)) => Ok(self.resolve_var(&name)),
            Some(Token::LParen) => {
                let inner = self.parse_comma()?;
                if self.next_tok() != Some(Token::RParen) {
                    return Err("Expected ')' to close parenthesis".to_string());
                }
                Ok(inner)
            }
            other => Err(format!("Expected arithmetic operand, got {:?}", other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_arithmetic() {
        let env = Env::for_command();
        assert_eq!(eval_arithmetic_expr("1 + 2 * 3", &env).unwrap(), 7);
        assert_eq!(eval_arithmetic_expr("(1 + 2) * 3", &env).unwrap(), 9);
        assert_eq!(eval_arithmetic_expr("10 - 4 - 2", &env).unwrap(), 4);
        assert_eq!(eval_arithmetic_expr("2 ** 3", &env).unwrap(), 8);
        assert_eq!(eval_arithmetic_expr("10 % 3", &env).unwrap(), 1);
    }

    #[test]
    fn test_variables_and_assignment() {
        let env = Env::for_command();
        env.vars.write().insert("x".to_string(), Val::Int(5));
        assert_eq!(eval_arithmetic_expr("x + 3", &env).unwrap(), 8);
        assert_eq!(eval_arithmetic_expr("$x + 3", &env).unwrap(), 8);
        assert_eq!(eval_arithmetic_expr("x = x + 1", &env).unwrap(), 6);
        assert_eq!(env.vars.read().get("x"), Some(&Val::Int(6)));
    }

    #[test]
    fn test_ternary_and_logical() {
        let env = Env::for_command();
        assert_eq!(eval_arithmetic_expr("1 ? 42 : 100", &env).unwrap(), 42);
        assert_eq!(eval_arithmetic_expr("0 ? 42 : 100", &env).unwrap(), 100);
        assert_eq!(eval_arithmetic_expr("5 > 3 && 2 < 4", &env).unwrap(), 1);
        assert_eq!(eval_arithmetic_expr("5 == 5", &env).unwrap(), 1);
        assert_eq!(eval_arithmetic_expr("5 != 5", &env).unwrap(), 0);
    }

    #[test]
    fn test_hex_octal_base() {
        let env = Env::for_command();
        assert_eq!(eval_arithmetic_expr("0x10", &env).unwrap(), 16);
        assert_eq!(eval_arithmetic_expr("010", &env).unwrap(), 8);
        assert_eq!(eval_arithmetic_expr("2#1010", &env).unwrap(), 10);
    }
}
