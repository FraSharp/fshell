// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! The native capability table: POSIX semantics native deliberately does not
//! provide, and therefore the inputs auto mode must divert.
//!
//! Its job is narrow, and worth stating precisely: prevent *silent semantic
//! misrouting* — input native accepts but cannot give POSIX meaning. It is not
//! a compatibility analyzer, it does not guess intent beyond the requirement
//! itself, and it is not where native bugs live. `$?` is native-owned and stays
//! native even though native computes it wrongly: that is a fix, not a
//! diversion. A table built from native defects would decay into "prefer POSIX
//! whenever possible" and quietly defeat having native syntax at all.
//!
//! The table grows from observed divergence, one row at a time. A construct
//! absent from it runs natively, and that is the intended default.
//!
//! Deliberately *not* here, with reasons:
//!
//! * **Field splitting of ordinary variables.** `$BAR` where `BAR="a b"` splits
//!   in POSIX and not in native, but the difference depends on the *value*,
//!   which the router cannot see: `$HOME` is the same word and never splits.
//!   Diverting every unquoted expansion would hijack native's own words — a
//!   `Val::List` in argv is deliberately one argument — to fix a case that
//!   cannot be identified statically. The divergence stays recorded in the
//!   suite as a semantic known failure rather than an armed routing row.
//!   `"$@"`/`$*` are different: native has no such parameter at all, so they are
//!   [`PosixRequirement::PosixSpecialParameter`] regardless of any value.
//! * **Process substitution.** Native implements it, so it owns it.
//! * **`$?`.** Native-owned; see above.

use fshell_core::QuoteKind;
use fshell_core::ast::{Expr, Pipeline, PipelineStage, Stmt, StringPart};

/// A POSIX semantic native deliberately does not provide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PosixRequirement {
    /// The positional-parameter lists `$@` and `$*`.
    ///
    /// Native has no concept of them, so `"$@"` does not expand to the
    /// parameters at all. `$?` is deliberately absent: native models it and
    /// merely gets it wrong.
    PosixSpecialParameter,
    /// `~user`. Native expands `~` and `~/` only, so a bare `~user` reaches
    /// argv unexpanded.
    PosixTildeUser,
}

impl PosixRequirement {
    /// Name used in the routing trace and asserted by the conformance suite.
    pub fn label(self) -> &'static str {
        match self {
            PosixRequirement::PosixSpecialParameter => "PosixSpecialParameter",
            PosixRequirement::PosixTildeUser => "PosixTildeUser",
        }
    }
}

/// The first POSIX-only requirement `stmts` needs, in source order.
///
/// `None` means native owns this input. Reporting only the first requirement
/// keeps the recorded reason stable and meaningful: one input, one reason.
pub fn required_posix_semantics(stmts: &[Stmt]) -> Option<PosixRequirement> {
    let mut found = None;
    for stmt in stmts {
        visit_stmt(stmt, &mut found);
        if found.is_some() {
            break;
        }
    }
    found
}

/// Whether `name` is a parameter native cannot model.
///
/// Only the positional-parameter lists qualify. `$?`, `$#`, `$!`, `$-`, `$$`
/// and the positional `$1`.. are *not* listed: each would need its own row
/// justified by an observed divergence, and `$?` in particular is native-owned.
fn is_unmodelled_parameter(name: &str) -> bool {
    matches!(name, "@" | "*")
}

/// Whether an unquoted word is a `~user` prefix.
///
/// Quoting matters as much as it does in globbing: `'~root'` is literal in
/// POSIX too, so only an unquoted `~` followed by anything but `/` qualifies.
/// `~` and `~/…` are native tilde forms and stay native.
fn is_tilde_user(text: &str) -> bool {
    match text.strip_prefix('~') {
        Some(rest) => !rest.is_empty() && !rest.starts_with('/'),
        None => false,
    }
}

/// Whether the word's first fragment is an unquoted `~user`.
fn word_has_tilde_user(parts: &[StringPart]) -> bool {
    matches!(
        parts.first(),
        Some(StringPart::Lit { text, quote })
            if *quote == QuoteKind::Unquoted && is_tilde_user(text)
    )
}

fn record(found: &mut Option<PosixRequirement>, requirement: PosixRequirement) {
    if found.is_none() {
        *found = Some(requirement);
    }
}

// The walk covers command arguments, assignments, control-flow bodies and the
// expressions nested inside them — enough to reach every word and parameter an
// input can use. Constructs it does not descend into hold no command words:
// comments, arithmetic strings, raw string literals and `PosixBlock` bodies
// (which run under the POSIX engine by definition, so there is nothing to
// divert).

fn visit_stmt(stmt: &Stmt, found: &mut Option<PosixRequirement>) {
    if found.is_some() {
        return;
    }
    match stmt {
        Stmt::Local { expr, .. } => {
            if let Some(expr) = expr {
                visit_expr(expr, found);
            }
        }
        Stmt::Let { expr, .. }
        | Stmt::Assign { expr, .. }
        | Stmt::Update { expr, .. }
        | Stmt::Source { path: expr, .. }
        | Stmt::Return(expr) => visit_expr(expr, found),
        Stmt::Exit(expr) => {
            if let Some(expr) = expr {
                visit_expr(expr, found);
            }
        }
        Stmt::While {
            condition, body, ..
        } => {
            visit_expr(condition, found);
            visit_stmts(body, found);
        }
        Stmt::For { iter, body, .. } => {
            visit_expr(iter, found);
            visit_stmts(body, found);
        }
        Stmt::FnDef { body, .. }
        | Stmt::Unsafe { body }
        | Stmt::ReactiveCellEvery { body, .. }
        | Stmt::Every { body, .. } => visit_stmts(body, found),
        Stmt::TryCatch {
            try_body,
            catch_body,
            ..
        } => {
            visit_stmts(try_body, found);
            visit_stmts(catch_body, found);
        }
        Stmt::Match { expr, arms } => {
            visit_expr(expr, found);
            for arm in arms {
                visit_stmts(&arm.body, found);
            }
        }
        Stmt::WithCaps { caps, body } => {
            for cap in caps {
                visit_expr(cap, found);
            }
            visit_stmts(body, found);
        }
        Stmt::ReactiveCell { pipeline, .. } => visit_pipeline(pipeline, found),
        Stmt::Background(inner) | Stmt::Spanned { stmt: inner, .. } => visit_stmt(inner, found),
        Stmt::And(left, right) | Stmt::Or(left, right) => {
            visit_stmt(left, found);
            visit_stmt(right, found);
        }
        Stmt::Expr(expr) => visit_expr(expr, found),
        Stmt::Break
        | Stmt::Continue
        | Stmt::Comment(_)
        | Stmt::On { .. }
        | Stmt::PosixBlock { .. } => {}
    }
}

fn visit_stmts(stmts: &[Stmt], found: &mut Option<PosixRequirement>) {
    for stmt in stmts {
        visit_stmt(stmt, found);
        if found.is_some() {
            return;
        }
    }
}

fn visit_expr(expr: &Expr, found: &mut Option<PosixRequirement>) {
    if found.is_some() {
        return;
    }
    match expr {
        Expr::Variable(name) | Expr::VarWithModifier { name, .. } => {
            if is_unmodelled_parameter(name) {
                record(found, PosixRequirement::PosixSpecialParameter);
            }
        }
        Expr::String(parts) => {
            if word_has_tilde_user(parts) {
                record(found, PosixRequirement::PosixTildeUser);
            }
            for part in parts {
                if let StringPart::Expr { expr, .. } = part {
                    visit_expr(expr, found);
                }
            }
        }
        // A bare word is unquoted by definition.
        Expr::Ident(word) => {
            if is_tilde_user(word) {
                record(found, PosixRequirement::PosixTildeUser);
            }
        }
        Expr::List(items) => {
            for item in items {
                visit_expr(item, found);
            }
        }
        Expr::Map(pairs) => {
            for (_, value) in pairs {
                visit_expr(value, found);
            }
        }
        Expr::BinaryOp { lhs, rhs, .. } => {
            visit_expr(lhs, found);
            visit_expr(rhs, found);
        }
        Expr::Not(inner)
        | Expr::MemberAccess { expr: inner, .. }
        | Expr::ArithmeticExpansion(inner)
        | Expr::Spanned { expr: inner, .. } => visit_expr(inner, found),
        Expr::Pipeline(pipeline) | Expr::InlinePipeline(pipeline) => {
            visit_pipeline(pipeline, found)
        }
        // The body is a statement list; the commands inside it are native, but
        // their words can still carry a POSIX-only form.
        Expr::Substitution(stmts) => visit_stmts(stmts, found),
        Expr::If {
            condition,
            then_body,
            else_body,
        } => {
            visit_expr(condition, found);
            visit_stmts(then_body, found);
            if let Some(body) = else_body {
                visit_stmts(body, found);
            }
        }
        // Native owns process substitution, so its pipeline is not a diversion;
        // the commands inside it are still ordinary words, though.
        Expr::ProcessSubst { pipeline, .. } => visit_pipeline(pipeline, found),
        Expr::Null
        | Expr::Bool(_)
        | Expr::Int(_)
        | Expr::Float(_)
        | Expr::AnsiCQuote(_)
        | Expr::RawMultiLineString(_)
        | Expr::MultiLineString { .. } => {}
    }
}

fn visit_pipeline(pipeline: &Pipeline, found: &mut Option<PosixRequirement>) {
    for stage in &pipeline.stages {
        visit_stage(stage, found);
        if found.is_some() {
            return;
        }
    }
}

fn visit_stage(stage: &PipelineStage, found: &mut Option<PosixRequirement>) {
    match stage {
        PipelineStage::CommandCall { args, env, .. } => {
            for arg in args {
                visit_expr(arg, found);
            }
            for (_, value) in env {
                visit_expr(value, found);
            }
        }
        PipelineStage::Filter { condition } => visit_expr(condition, found),
        PipelineStage::Map { projections } => {
            for projection in projections {
                visit_expr(projection, found);
            }
        }
        PipelineStage::BoundaryOperator { .. }
        | PipelineStage::Count
        | PipelineStage::FdRedirect { .. }
        | PipelineStage::Grep { .. }
        | PipelineStage::Hash { .. }
        | PipelineStage::Heredoc { .. }
        | PipelineStage::HereString { .. }
        | PipelineStage::Limit { .. }
        | PipelineStage::Mark { .. }
        | PipelineStage::Read { .. }
        | PipelineStage::Sort { .. }
        | PipelineStage::Traverse { .. }
        | PipelineStage::Write { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fshell_core::Parser;

    fn requirement(script: &str) -> Option<PosixRequirement> {
        let stmts = Parser::new(script)
            .parse_statements()
            .unwrap_or_else(|e| panic!("{script:?} should parse: {e}"));
        required_posix_semantics(&stmts)
    }

    #[test]
    fn positional_parameter_lists_need_posix() {
        assert_eq!(
            requirement("echo \"$@\""),
            Some(PosixRequirement::PosixSpecialParameter)
        );
        assert_eq!(
            requirement("echo $@"),
            Some(PosixRequirement::PosixSpecialParameter)
        );
        assert_eq!(
            requirement("echo \"$*\""),
            Some(PosixRequirement::PosixSpecialParameter)
        );
        assert_eq!(
            requirement("for f in $@ { echo hi }"),
            Some(PosixRequirement::PosixSpecialParameter)
        );
    }

    #[test]
    fn status_parameter_stays_native() {
        // Native owns `$?`; it computes it wrongly, which is a fix, not a
        // diversion. Routing it would hide the bug the suite tracks.
        assert_eq!(requirement("false; emit --stdout \"rc=$?\""), None);
        assert_eq!(requirement("emit --exit 3; echo $?"), None);
    }

    #[test]
    fn ordinary_variables_stay_native() {
        assert_eq!(requirement("argvdump $BAR"), None);
        assert_eq!(requirement("echo \"$HOME\""), None);
        assert_eq!(requirement("let x = 5; echo $x"), None);
        assert_eq!(requirement("argvdump abc"), None);
    }

    #[test]
    fn tilde_user_needs_posix() {
        assert_eq!(
            requirement("echo ~root"),
            Some(PosixRequirement::PosixTildeUser)
        );
        assert_eq!(
            requirement("argvdump ~root/x"),
            Some(PosixRequirement::PosixTildeUser)
        );
    }

    #[test]
    fn native_tilde_forms_stay_native() {
        assert_eq!(requirement("argvdump ~"), None);
        assert_eq!(requirement("argvdump ~/src"), None);
        assert_eq!(requirement("echo a~b"), None);
    }

    #[test]
    fn quoted_tilde_is_not_a_tilde_prefix() {
        // POSIX does not expand a quoted `~` either, so this is native's too.
        assert_eq!(requirement("argvdump '~root'"), None);
        assert_eq!(requirement("argvdump \"~root\""), None);
    }

    #[test]
    fn process_substitution_stays_native() {
        assert_eq!(requirement("argvdump <(echo hi)"), None);
    }

    #[test]
    fn first_requirement_in_source_order_wins() {
        assert_eq!(
            requirement("echo ~root; echo \"$@\""),
            Some(PosixRequirement::PosixTildeUser)
        );
        assert_eq!(
            requirement("echo \"$@\"; echo ~root"),
            Some(PosixRequirement::PosixSpecialParameter)
        );
    }
}
