// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use crate::val_type_precedence;
use fshell_core::{
    BinOp, Expr, ParamModifier, PipelineStage, ProcessSubstDirection, SerializationFormat, Val,
};

pub fn cmp_vals(a: &Val, b: &Val) -> std::cmp::Ordering {
    let prec_a = val_type_precedence(a);
    let prec_b = val_type_precedence(b);
    if prec_a != prec_b {
        return prec_a.cmp(&prec_b);
    }
    match (a, b) {
        (Val::Null, Val::Null) => std::cmp::Ordering::Equal,
        (Val::Bool(x), Val::Bool(y)) => x.cmp(y),
        (Val::Int(x), Val::Int(y)) => x.cmp(y),
        (Val::Float(x), Val::Float(y)) => {
            if x.is_nan() && y.is_nan() {
                std::cmp::Ordering::Equal
            } else if x.is_nan() {
                std::cmp::Ordering::Less
            } else if y.is_nan() {
                std::cmp::Ordering::Greater
            } else {
                x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal)
            }
        }
        (Val::String(x), Val::String(y)) => x.cmp(y),
        (Val::DateTime(x), Val::DateTime(y)) => x.cmp(y),
        (Val::Blob(x), Val::Blob(y)) => x.cmp(y),
        (Val::List(x), Val::List(y)) => x.len().cmp(&y.len()).then_with(|| {
            for (item_a, item_b) in x.iter().zip(y.iter()) {
                let c = cmp_vals(item_a, item_b);
                if c != std::cmp::Ordering::Equal {
                    return c;
                }
            }
            std::cmp::Ordering::Equal
        }),
        (Val::Map(x), Val::Map(y)) => x.len().cmp(&y.len()),
        (Val::Capability(x), Val::Capability(y)) => format!("{:?}", x).cmp(&format!("{:?}", y)),
        _ => std::cmp::Ordering::Equal,
    }
}

pub fn format_pipeline(pipeline: &fshell_core::Pipeline) -> String {
    let mut parts = Vec::new();
    for stage in &pipeline.stages {
        match stage {
            PipelineStage::CommandCall { name, args, .. } => {
                let mut s = name.clone();
                for arg in args {
                    s.push_str(&format!(" {}", format_expr(arg)));
                }
                parts.push(s);
            }
            PipelineStage::Filter { condition } => {
                parts.push(format!("filter {}", format_expr(condition)));
            }
            PipelineStage::Map { projections } => {
                let proj_strs: Vec<String> = projections.iter().map(format_expr).collect();
                parts.push(format!("map {}", proj_strs.join(" ")));
            }
            PipelineStage::Sort { column, descending } => {
                if *descending {
                    parts.push(format!("sort {} desc", column));
                } else {
                    parts.push(format!("sort {}", column));
                }
            }
            PipelineStage::Grep { pattern } => {
                parts.push(format!("grep {}", format_expr(pattern)));
            }
            PipelineStage::Mark { pattern } => {
                parts.push(format!("mark {}", format_expr(pattern)));
            }
            PipelineStage::Count => {
                parts.push("count".to_string());
            }
            PipelineStage::Hash { mode, per_record } => {
                let mut s = "hash".to_string();
                match mode {
                    fshell_core::HashMode::Hash256 => s.push_str(" -a 256"),
                    fshell_core::HashMode::Hash512 => s.push_str(" -a 512"),
                    fshell_core::HashMode::Xof(len) => s.push_str(&format!(" -a xof -o {}", len)),
                }
                if *per_record {
                    s.push_str(" --per-record");
                }
                parts.push(s);
            }
            PipelineStage::Limit { amount } => {
                parts.push(format!("limit {}", format_expr(amount)));
            }
            PipelineStage::BoundaryOperator { format } => {
                let f_str = match format {
                    SerializationFormat::Json => "@json",
                    SerializationFormat::Yaml => "@yaml",
                    SerializationFormat::MsgPack => "@msgpack",
                    SerializationFormat::Text => "@text",
                    SerializationFormat::Csv => "@csv",
                    SerializationFormat::Table => "@table",
                    SerializationFormat::Bar => "@bar",
                };
                parts.push(f_str.to_string());
            }
            PipelineStage::Traverse { edge_label } => {
                parts.push(format!("traverse {}", format_expr(edge_label)));
            }
            PipelineStage::Write {
                path,
                append,
                redirect_stdout,
                redirect_stderr,
            } => {
                let op = match (redirect_stdout, redirect_stderr, append) {
                    (true, true, true) => "&>>",
                    (true, true, false) => "&>",
                    (true, false, true) => ">>",
                    (true, false, false) => ">",
                    (false, true, true) => "2>>",
                    (false, true, false) => "2>",
                    (false, false, _) => ">",
                };
                parts.push(format!("{} {}", op, format_expr(path)));
            }
            PipelineStage::Read { path } => {
                parts.push(format!("< {}", format_expr(path)));
            }
            PipelineStage::FdRedirect { src_fd, dst_fd } => {
                if *dst_fd < 0 {
                    parts.push(format!("{}>&-", src_fd));
                } else {
                    parts.push(format!("{}>&{}", src_fd, dst_fd));
                }
            }
            PipelineStage::Heredoc {
                delimiter,
                strip_tabs,
                quoted,
                ..
            } => {
                let prefix = if *strip_tabs { "<<-" } else { "<<" };
                let delim_str = if *quoted {
                    format!("'{}'", delimiter)
                } else {
                    delimiter.clone()
                };
                parts.push(format!("{}{}", prefix, delim_str));
            }
            PipelineStage::HereString { content } => {
                parts.push(format!("<<< {}", format_expr(content)));
            }
        }
    }
    parts.join(" | ")
}

pub fn format_expr(expr: &Expr) -> String {
    match expr {
        Expr::Spanned { expr: inner, .. } => format_expr(inner),
        Expr::Null => "null".to_string(),
        Expr::Bool(b) => b.to_string(),
        Expr::Int(i) => i.to_string(),
        Expr::Float(f) => f.to_string(),
        Expr::String(parts) => {
            let mut s = String::new();
            s.push('"');
            for part in parts {
                match part {
                    fshell_core::StringPart::Lit(l) => s.push_str(l),
                    fshell_core::StringPart::Expr(e) => {
                        s.push_str(&format!("{{{}}}", format_expr(e)));
                    }
                }
            }
            s.push('"');
            s
        }
        Expr::Ident(id) => id.clone(),
        Expr::List(list) => {
            let item_strs: Vec<String> = list.iter().map(format_expr).collect();
            format!("[{}]", item_strs.join(", "))
        }
        Expr::Map(map) => {
            let entry_strs: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("\"{}\": {}", k, format_expr(v)))
                .collect();
            format!("{{{}}}", entry_strs.join(", "))
        }
        Expr::Variable(var) => format!("${}", var),
        Expr::BinaryOp { op, lhs, rhs } => {
            let op_str = match op {
                BinOp::Add => "+",
                BinOp::Sub => "-",
                BinOp::Mul => "*",
                BinOp::Div => "/",
                BinOp::Eq => "==",
                BinOp::Neq => "!=",
                BinOp::Lt => "<",
                BinOp::Lte => "<=",
                BinOp::Gt => ">",
                BinOp::Gte => ">=",
                BinOp::ReMatch => "~",
                BinOp::And => "and",
                BinOp::Or => "or",
            };
            format!("({} {} {})", format_expr(lhs), op_str, format_expr(rhs))
        }
        Expr::MemberAccess { expr, member } => {
            format!("{}.{}", format_expr(expr), member)
        }
        Expr::Pipeline(p) => format_pipeline(p),
        Expr::InlinePipeline(p) => format!("$| {} |", format_pipeline(p)),
        Expr::VarWithModifier { name, modifier } => match modifier {
            ParamModifier::Tail => format!("${{{}:t}}", name),
            ParamModifier::Head => format!("${{{}:h}}", name),
            ParamModifier::Root => format!("${{{}:r}}", name),
            ParamModifier::Ext => format!("${{{}:e}}", name),
            ParamModifier::Default(e) => format!("${{{}:-{}}}", name, format_expr(e)),
            ParamModifier::AssignDefault(e) => format!("${{{}:={}}}", name, format_expr(e)),
            ParamModifier::ErrorIfUnset(e) => format!("${{{}:?{}}}", name, format_expr(e)),
            ParamModifier::Alternate(e) => format!("${{{}:+{}}}", name, format_expr(e)),
            ParamModifier::Substring { offset, length } => match length {
                Some(l) => format!("${{{}:{}:{}}}", name, offset, l),
                None => format!("${{{}:{}}}", name, offset),
            },
            ParamModifier::ShortestPrefix(p) => format!("${{{}#{}}}", name, format_expr(p)),
            ParamModifier::LongestPrefix(p) => format!("${{{}##{}}}", name, format_expr(p)),
            ParamModifier::ShortestSuffix(p) => format!("${{{}%{}}}", name, format_expr(p)),
            ParamModifier::LongestSuffix(p) => format!("${{{}%%{}}}", name, format_expr(p)),
            ParamModifier::Replace {
                pattern,
                replacement,
                global,
            } => {
                let sep = if *global { "//" } else { "/" };
                format!(
                    "${{{}{}{}/{}}}",
                    name,
                    sep,
                    format_expr(pattern),
                    format_expr(replacement)
                )
            }
            ParamModifier::Upper => format!("${{{}:u}}", name),
            ParamModifier::Lower => format!("${{{}:l}}", name),
            ParamModifier::StringLength => format!("${{#{}}}", name),
        },
        Expr::ProcessSubst {
            direction,
            pipeline,
        } => {
            let arrow = match direction {
                ProcessSubstDirection::Input => '<',
                ProcessSubstDirection::Output => '>',
            };
            format!("{}({})", arrow, format_pipeline(pipeline))
        }
        Expr::If {
            condition,
            then_body: _,
            else_body,
        } => {
            let mut s = format!("if {} {{ ... }}", format_expr(condition));
            if else_body.is_some() {
                s.push_str(" else { ... }");
            }
            s
        }
        Expr::Not(inner) => format!("!{}", format_expr(inner)),
        Expr::ArithmeticExpansion(inner) => format!("$(({}))", format_expr(inner)),
        Expr::AnsiCQuote(s) => format!("$'{}'", s),
        Expr::RawMultiLineString(s) => {
            if s.contains('\n') {
                format!("'''\n{}'''", s)
            } else {
                s.clone()
            }
        }
        Expr::MultiLineString { parts, .. } => {
            let mut s = String::new();
            s.push_str("<<");
            for part in parts {
                match part {
                    fshell_core::StringPart::Lit(l) => s.push_str(l),
                    fshell_core::StringPart::Expr(e) => {
                        s.push_str(&format!("{{{}}}", format_expr(e)));
                    }
                }
            }
            s
        }
    }
}
