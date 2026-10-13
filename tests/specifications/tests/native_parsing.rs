//! Complete native ASTs, quoting provenance, byte spans and classified failures.
//! No shell artifact, host state, snapshots or evaluator are needed here.

use fshell_core::{
    BinOp, Expr, ParseError, Parser, Pipeline, PipelineStage, QuoteKind, Stmt, StringPart,
};
use std::ops::Range;

fn expression(expr: Expr, bytes: Range<usize>) -> Expr {
    Expr::Spanned {
        expr: Box::new(expr),
        span: (bytes.start, bytes.len()).into(),
    }
}

fn statement(stmt: Stmt, bytes: Range<usize>) -> Stmt {
    Stmt::Spanned {
        stmt: Box::new(stmt),
        span: (bytes.start, bytes.len()).into(),
    }
}

fn expression_statement(expr: Expr, bytes: Range<usize>) -> Stmt {
    statement(Stmt::Expr(expression(expr, bytes.clone())), bytes)
}

fn binary(op: BinOp, lhs: Expr, rhs: Expr) -> Expr {
    Expr::BinaryOp {
        op,
        lhs: Box::new(lhs),
        rhs: Box::new(rhs),
    }
}

fn literal(text: &str, quote: QuoteKind) -> Expr {
    Expr::String(vec![StringPart::Lit {
        text: text.into(),
        quote,
    }])
}

fn command(name: &str, args: Vec<Expr>, bytes: Range<usize>) -> PipelineStage {
    PipelineStage::CommandCall {
        name: name.into(),
        args,
        env: Vec::new(),
        span: (bytes.start, bytes.len()).into(),
    }
}

fn pipeline(source: &str) -> Pipeline {
    let mut parser = Parser::new(source);
    let result = parser.parse_pipeline().expect("complete valid pipeline");
    assert!(
        parser
            .parse_statements()
            .expect("remaining input")
            .is_empty(),
        "pipeline parser must consume the fixture"
    );
    result
}

#[test]
fn arithmetic_precedence_and_left_associativity_have_distinct_complete_trees() {
    let source = "2 + 3 * 4";
    assert_eq!(
        Parser::new(source).parse_statements().expect("precedence"),
        [expression_statement(
            binary(
                BinOp::Add,
                Expr::Int(2),
                binary(BinOp::Mul, Expr::Int(3), Expr::Int(4))
            ),
            0..source.len()
        )]
    );
    let source = "9 - 3 - 1";
    assert_eq!(
        Parser::new(source)
            .parse_statements()
            .expect("associativity"),
        [expression_statement(
            binary(
                BinOp::Sub,
                binary(BinOp::Sub, Expr::Int(9), Expr::Int(3)),
                Expr::Int(1)
            ),
            0..source.len()
        )]
    );
}

#[test]
fn parentheses_change_grouping_and_retain_the_inner_source_span() {
    let source = "(2 + 3) * 4";
    assert_eq!(
        Parser::new(source)
            .parse_statements()
            .expect("grouped expression"),
        [expression_statement(
            binary(
                BinOp::Mul,
                expression(binary(BinOp::Add, Expr::Int(2), Expr::Int(3)), 1..6),
                Expr::Int(4)
            ),
            0..source.len()
        )]
    );
}

#[test]
fn boolean_operators_in_a_declaration_are_value_operators_not_status_chains() {
    let source = "let accept=true && false || true";
    assert_eq!(
        Parser::new(source)
            .parse_statements()
            .expect("boolean declaration"),
        [statement(
            Stmt::Let {
                name: "accept".into(),
                ty: None,
                expr: expression(
                    binary(
                        BinOp::Or,
                        binary(BinOp::And, Expr::Bool(true), Expr::Bool(false)),
                        Expr::Bool(true)
                    ),
                    11..source.len()
                )
            },
            0..source.len()
        )]
    );
}

#[test]
fn statement_level_and_or_chain_command_statuses_in_source_order() {
    let source = "left&&middle||right";
    let cmd = |name, bytes: Range<usize>| {
        expression_statement(
            Expr::Pipeline(Pipeline {
                stages: vec![command(name, Vec::new(), bytes.clone())],
                boundaries: Vec::new(),
            }),
            bytes,
        )
    };
    assert_eq!(
        Parser::new(source)
            .parse_statements()
            .expect("status chain"),
        [Stmt::Or(
            Box::new(Stmt::And(
                Box::new(cmd("left", 0..4)),
                Box::new(cmd("middle", 6..12))
            )),
            Box::new(cmd("right", 14..19))
        )]
    );
}

#[test]
fn keyword_prefixes_remain_assignment_names() {
    let source = "letter=1;letdown=2";
    assert_eq!(
        Parser::new(source)
            .parse_statements()
            .expect("assignment names"),
        [
            statement(
                Stmt::Assign {
                    name: "letter".into(),
                    expr: Expr::Int(1)
                },
                0..8
            ),
            statement(
                Stmt::Assign {
                    name: "letdown".into(),
                    expr: Expr::Int(2)
                },
                9..18
            ),
        ]
    );
}

#[test]
fn structured_map_and_list_preserve_order_types_and_nested_spans() {
    let source = r#"{"a":[1,2],"b":true}"#;
    let map = Expr::Map(vec![
        (
            "a".into(),
            expression(
                Expr::List(vec![
                    expression(Expr::Int(1), 6..7),
                    expression(Expr::Int(2), 8..9),
                ]),
                5..10,
            ),
        ),
        ("b".into(), expression(Expr::Bool(true), 15..19)),
    ]);
    assert_eq!(
        Parser::new(source)
            .parse_statements()
            .expect("structured value"),
        [expression_statement(map, 0..20)]
    );
}

#[test]
fn unicode_source_spans_count_bytes_instead_of_characters() {
    let source = "['α🙂',9]";
    assert_eq!(source.len(), 12);
    assert_eq!(
        Parser::new(source)
            .parse_statements()
            .expect("Unicode list"),
        [expression_statement(
            Expr::List(vec![
                expression(literal("α🙂", QuoteKind::Single), 1..9),
                expression(Expr::Int(9), 10..11)
            ]),
            0..12
        )]
    );
}

#[test]
fn quoted_command_literals_are_not_coerced_to_scalar_types() {
    let source = "echo 1 true null '1' 'true' 'null'";
    assert_eq!(
        pipeline(source),
        Pipeline {
            stages: vec![command(
                "echo",
                vec![
                    Expr::Int(1),
                    Expr::Bool(true),
                    Expr::Null,
                    literal("1", QuoteKind::Single),
                    literal("true", QuoteKind::Single),
                    literal("null", QuoteKind::Single)
                ],
                0..source.len()
            )],
            boundaries: Vec::new()
        }
    );
}

#[test]
fn adjacent_word_fragments_keep_individual_quote_provenance() {
    let source = "echo foo'*'\"?\"";
    assert_eq!(
        pipeline(source),
        Pipeline {
            stages: vec![command(
                "echo",
                vec![Expr::String(vec![
                    StringPart::Lit {
                        text: "foo".into(),
                        quote: QuoteKind::Unquoted
                    },
                    StringPart::Lit {
                        text: "*".into(),
                        quote: QuoteKind::Single
                    },
                    StringPart::Lit {
                        text: "?".into(),
                        quote: QuoteKind::Double
                    },
                ])],
                0..source.len()
            )],
            boundaries: Vec::new()
        }
    );
}

#[test]
fn variable_interpolation_is_enabled_only_in_double_quoted_fragments() {
    let source = "echo '$value' \"$value\"";
    assert_eq!(
        pipeline(source),
        Pipeline {
            stages: vec![command(
                "echo",
                vec![
                    literal("$value", QuoteKind::Single),
                    Expr::String(vec![StringPart::Expr {
                        expr: Box::new(Expr::Variable("value".into())),
                        quote: QuoteKind::Double
                    }]),
                ],
                0..source.len()
            )],
            boundaries: Vec::new()
        }
    );
}

#[test]
fn quoted_json_remains_one_argument_while_unquoted_braces_expand() {
    let source = r#"echo '{"a":[1,2]}'"#;
    assert_eq!(
        pipeline(source),
        Pipeline {
            stages: vec![command(
                "echo",
                vec![literal(r#"{"a":[1,2]}"#, QuoteKind::Single)],
                0..source.len()
            )],
            boundaries: Vec::new()
        }
    );
    let source = "echo {a,b}";
    assert_eq!(
        pipeline(source),
        Pipeline {
            stages: vec![command(
                "echo",
                vec![
                    literal("a", QuoteKind::Unquoted),
                    literal("b", QuoteKind::Unquoted)
                ],
                0..source.len()
            )],
            boundaries: Vec::new()
        }
    );
}

#[test]
fn redirection_positions_distinguish_left_and_right_pipeline_segments() {
    let write = || PipelineStage::Write {
        path: literal("out.txt", QuoteKind::Unquoted),
        append: false,
        redirect_stdout: true,
        redirect_stderr: false,
    };
    assert_eq!(
        pipeline("echo>out.txt|cat"),
        Pipeline {
            stages: vec![
                command("echo", Vec::new(), 0..4),
                write(),
                command("cat", Vec::new(), 13..16)
            ],
            boundaries: vec![2]
        }
    );
    assert_eq!(
        pipeline("echo|>out.txt cat"),
        Pipeline {
            stages: vec![
                command("echo", Vec::new(), 0..4),
                write(),
                command("cat", Vec::new(), 14..17)
            ],
            boundaries: vec![1]
        }
    );
}

#[test]
fn descriptor_duplication_and_file_redirection_preserve_source_order() {
    let write = || PipelineStage::Write {
        path: literal("out.txt", QuoteKind::Unquoted),
        append: false,
        redirect_stdout: true,
        redirect_stderr: false,
    };
    let duplicate = || PipelineStage::FdRedirect {
        src_fd: 2,
        dst_fd: 1,
    };
    assert_eq!(
        pipeline("echo>out.txt 2>&1"),
        Pipeline {
            stages: vec![command("echo", Vec::new(), 0..4), write(), duplicate()],
            boundaries: Vec::new()
        }
    );
    assert_eq!(
        pipeline("echo 2>&1>out.txt"),
        Pipeline {
            stages: vec![command("echo", Vec::new(), 0..5), duplicate(), write()],
            boundaries: Vec::new()
        }
    );
}

#[test]
fn native_redirection_identifiers_are_values_and_quoted_targets_are_literals() {
    let write = |path| PipelineStage::Write {
        path,
        append: false,
        redirect_stdout: true,
        redirect_stderr: false,
    };
    assert_eq!(
        pipeline("echo>out"),
        Pipeline {
            stages: vec![
                command("echo", Vec::new(), 0..4),
                write(Expr::Ident("out".into()))
            ],
            boundaries: Vec::new()
        }
    );
    assert_eq!(
        pipeline("echo>\"out\""),
        Pipeline {
            stages: vec![
                command("echo", Vec::new(), 0..4),
                write(literal("out", QuoteKind::Double))
            ],
            boundaries: Vec::new()
        }
    );
}

#[test]
fn malformed_delimiters_keep_error_identity_and_unicode_byte_locations() {
    let source = "let value='é';let bad=(1]";
    assert_eq!(
        Parser::new(source)
            .parse_statements()
            .expect_err("mismatched parenthesis"),
        ParseError::ExpectedChar {
            expected: ')',
            found: ']',
            span: (source.find(']').expect("fixture delimiter"), 1).into()
        }
    );
    let source = "{\"a\"Ω1}";
    assert_eq!(
        Parser::new(source)
            .parse_statements()
            .expect_err("missing map colon"),
        ParseError::ExpectedChar {
            expected: ':',
            found: 'Ω',
            span: (4, 2).into()
        }
    );
}

#[test]
fn missing_declaration_value_is_an_eof_error_at_the_input_boundary() {
    let source = "let bad=";
    assert_eq!(
        Parser::new(source)
            .parse_statements()
            .expect_err("incomplete declaration"),
        ParseError::UnexpectedEof {
            span: (source.len(), 1).into()
        }
    );
}
