// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use miette::SourceSpan;

/// An expression in the fshell AST.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Expr {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(Vec<StringPart>), // Supports interpolation: e.g. "Hello, {name}!"
    Ident(String), // Bare identifier — resolves to variable if defined, else string (bare word as string)
    List(Vec<Expr>),
    Map(Vec<(String, Expr)>),
    Variable(String),
    BinaryOp {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// Unary boolean negation: `!expr`
    Not(Box<Expr>),
    MemberAccess {
        expr: Box<Expr>,
        member: String,
    },
    Pipeline(Pipeline),
    /// A captured inline pipeline: $| cmd1 | cmd2 | — returns captured Val output.
    InlinePipeline(Pipeline),
    /// Parameter expansion with modifier: ${var:t}, ${var:h}, ${var:r}, ${var:e}
    VarWithModifier {
        name: String,
        modifier: ParamModifier,
    },
    /// Process substitution: <(command) or >(command)
    ProcessSubst {
        direction: ProcessSubstDirection,
        pipeline: Pipeline,
    },
    /// $((expr)) — arithmetic expansion
    ArithmeticExpansion(Box<Expr>),
    /// ANSI-C quoted string: $'hello\nworld' — escape sequences interpreted, no interpolation.
    AnsiCQuote(String),
    /// Raw multi-line string (''' or heredoc with quoted delimiter) — no interpolation, no dedent.
    RawMultiLineString(String),
    /// Multi-line string with dedent and optional interpolation.
    MultiLineString {
        parts: Vec<StringPart>,
        dedent: DedentMode,
    },
    If {
        condition: Box<Expr>,
        then_body: Vec<Stmt>,
        else_body: Option<Vec<Stmt>>,
    },
    Spanned {
        expr: Box<Expr>,
        #[serde(skip, default = "default_span")]
        span: SourceSpan,
    },
}

/// The body of an `on` signal handler — either an inline block or a function name.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum OnHandler {
    /// Inline block of statements.
    Block(Vec<Stmt>),
    /// Name of a registered function to call.
    FunctionName(String),
}

/// A segment of an interpolated string.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum StringPart {
    Lit(String),
    Expr(Box<Expr>),
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Neq,
    Lt,
    Lte,
    Gt,
    Gte,
    ReMatch,
    And,
    Or,
}

/// A pipeline of sequential steps.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Pipeline {
    pub stages: Vec<PipelineStage>,
}

/// Individual stage inside a pipeline.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PipelineStage {
    CommandCall {
        name: String,
        args: Vec<Expr>,
        env: Vec<(String, Expr)>,
        #[serde(skip, default = "default_span")]
        span: SourceSpan,
    },
    Filter {
        condition: Expr,
    },
    Map {
        projections: Vec<Expr>,
    },
    Sort {
        column: String,
        descending: bool,
    },
    Grep {
        pattern: Expr,
    },
    Mark {
        pattern: Expr,
    },
    Count,
    Limit {
        amount: Expr,
    },
    BoundaryOperator {
        format: SerializationFormat,
    },
    Traverse {
        edge_label: Expr,
    },
    Write {
        path: Expr,
        append: bool,
        redirect_stdout: bool,
        redirect_stderr: bool,
    },
    /// Redirect standard input from a file: `< path` or `0< path`.
    Read {
        path: Expr,
    },
    /// Redirect a file descriptor to another file descriptor: `N>&M` (e.g. `2>&1` for stderr→stdout).
    FdRedirect {
        /// Source file descriptor (1 for stdout, 2 for stderr).
        src_fd: i32,
        /// Destination file descriptor to duplicate to.
        dst_fd: i32,
    },
    /// Heredoc input: `<<DELIM` / `<<-DELIM` / `<<'DELIM'` — content is the resolved heredoc body.
    Heredoc {
        /// Delimiter word (e.g. `EOF`, `TOML`).
        delimiter: String,
        /// The evaluated body: `RawMultiLineString` if quoted, `MultiLineString` otherwise.
        content: Expr,
        /// Whether `<<-` was used (strip leading tabs from content lines).
        strip_tabs: bool,
        /// Whether the delimiter was quoted (no interpolation).
        quoted: bool,
    },
    /// Here-string input: `<<< word` — word is expanded and fed as stdin with trailing newline.
    HereString {
        content: Expr,
    },
    Hash {
        mode: HashMode,
        per_record: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HashMode {
    Hash256,
    Hash512,
    Xof(usize),
}

/// Supported boundary serialization formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SerializationFormat {
    Json,
    Yaml,
    MsgPack,
    Text,
    Csv,
    Table,
    Bar,
}

/// A statement in the fshell AST.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Stmt {
    Local {
        name: String,
        expr: Option<Expr>,
    },
    Let {
        name: String,
        expr: Expr,
    },
    Assign {
        name: String,
        expr: Expr,
    },
    Update {
        name: String,
        op: BinOp,
        expr: Expr,
    },
    FnDef {
        name: String,
        params: Vec<Param>,
        ret_type: Option<String>,
        body: Vec<Stmt>,
    },
    Match {
        expr: Expr,
        arms: Vec<MatchArm>,
    },
    TryCatch {
        try_body: Vec<Stmt>,
        catch_var: String,
        catch_body: Vec<Stmt>,
    },
    WithCaps {
        caps: Vec<Expr>,
        body: Vec<Stmt>,
    },
    ReactiveCell {
        name: String,
        pipeline: Pipeline,
    },
    ReactiveCellEvery {
        name: String,
        duration: u64,
        unit: TimeUnit,
        body: Vec<Stmt>,
    },
    Unsafe {
        body: Vec<Stmt>,
    },
    Source {
        path: Expr,
        bash: bool,
    },
    /// Inline POSIX shell block — `sh { ... }` / `posix { ... }` / `bash { ... }`
    /// The body is raw POSIX source executed via the fshell-posix engine against the same Env.
    PosixBlock {
        body: String,
    },
    While {
        condition: Expr,
        body: Vec<Stmt>,
    },
    /// `for <var> in <expr> { ... }`
    For {
        var: String,
        iter: Expr,
        body: Vec<Stmt>,
    },
    /// `break` — exits the nearest enclosing while/for loop.
    Break,
    /// `continue` — skips to the next iteration of the nearest enclosing loop.
    Continue,
    /// `return <expr>` — returns a value from a function body.
    Return(Expr),
    /// Exit the shell or script with an optional exit code.
    Exit(Option<Expr>),
    /// Register a signal/event handler (e.g., on exit { cleanup }, on sigint { ... }).
    On {
        signal: String,
        handler: OnHandler,
    },
    /// Run a statement in the background (async, non-blocking).
    Background(Box<Stmt>),
    Every {
        duration: u64,
        unit: TimeUnit,
        body: Vec<Stmt>,
    },
    And(Box<Stmt>, Box<Stmt>),
    Or(Box<Stmt>, Box<Stmt>),
    Comment(String),
    Expr(Expr),
    Spanned {
        stmt: Box<Stmt>,
        #[serde(skip, default = "default_span")]
        span: SourceSpan,
    },
}

/// Function parameter with optional structural type constraints.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Param {
    pub name: String,
    pub constraint: TypeConstraint,
}

/// Types of structural and gradual typing constraints.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TypeConstraint {
    Any,
    Primitive(String),
    Structural {
        fields: Vec<(String, TypeConstraint)>,
        rest: bool,
        alias: Option<String>,
    },
}

/// Pattern in structural pattern matching.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum MatchPattern {
    Wildcard,
    Literal(LiteralPattern),
    Map {
        fields: Vec<(String, MatchPattern)>,
        rest: bool,
    },
}

/// Basic pattern matching literal types.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum LiteralPattern {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
}

/// A single arm in a match expression.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MatchArm {
    pub pattern: MatchPattern,
    pub body: Vec<Stmt>,
}

/// How to handle leading whitespace in multi-line strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DedentMode {
    /// No dedenting — preserve all whitespace.
    None,
    /// Strip common leading whitespace from all lines.
    All,
    /// Strip leading tabs only.
    LeadingTabs,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TimeUnit {
    Second,
    Minute,
    Hour,
}

/// Parameter expansion modifiers for ${var:...} and ${var#...} etc.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ParamModifier {
    // Legacy path modifiers
    /// :t — tail (basename)
    Tail,
    /// :h — head (dirname)
    Head,
    /// :r — root (remove extension)
    Root,
    /// :e — extension
    Ext,

    // Colon-prefixed operators
    /// :- — default if unset/empty
    Default(Box<Expr>),
    /// := — assign default if unset/empty
    AssignDefault(Box<Expr>),
    /// :? — error if unset/empty
    ErrorIfUnset(Box<Expr>),
    /// :+ — alternate if set and non-empty
    Alternate(Box<Expr>),
    /// :N or :N:M — substring (char-based)
    Substring { offset: i64, length: Option<u64> },

    // Direct suffix operators (no colon)
    /// # — shortest prefix match
    ShortestPrefix(Box<Expr>),
    /// ## — longest prefix match
    LongestPrefix(Box<Expr>),
    /// % — shortest suffix match
    ShortestSuffix(Box<Expr>),
    /// %% — longest suffix match
    LongestSuffix(Box<Expr>),

    // Replace operators
    /// /pat/repl — replace first occurrence
    /// //pat/repl — replace all occurrences
    Replace {
        pattern: Box<Expr>,
        replacement: Box<Expr>,
        global: bool,
    },

    // Length operator
    /// ${#var} — string length
    StringLength,
    /// :u — uppercase transformation
    Upper,
    /// :l — lowercase transformation
    Lower,
}

/// Direction for process substitution: <(cmd) reads from cmd, >(cmd) writes to cmd.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProcessSubstDirection {
    Input,
    Output,
}

fn default_span() -> SourceSpan {
    SourceSpan::new(0.into(), 0)
}

impl Expr {
    pub fn unpack(&self) -> &Self {
        let mut current = self;
        while let Expr::Spanned { expr: inner, .. } = current {
            current = inner;
        }
        current
    }

    pub fn into_unpack(self) -> Self {
        match self {
            Expr::Spanned { expr: inner, .. } => inner.into_unpack(),
            other => other,
        }
    }
}

impl Stmt {
    pub fn unpack(&self) -> &Self {
        let mut current = self;
        while let Stmt::Spanned { stmt: inner, .. } = current {
            current = inner;
        }
        current
    }

    pub fn into_unpack(self) -> Self {
        match self {
            Stmt::Spanned { stmt: inner, .. } => inner.into_unpack(),
            other => other,
        }
    }
}
