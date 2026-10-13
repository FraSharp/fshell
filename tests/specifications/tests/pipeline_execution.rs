//! Production pipeline dispatch, transport, joining and statement finalization.
//! Registered async handlers expose controlled stage inputs/results; the engine
//! owns all inter-stage forks, cancellation propagation, routes and status slots.

use fshell_core::diagnostic::{ErrorCode, StderrStream};
use fshell_core::{Expr, FshDiag, Pipeline, PipelineStage, ShellError, Stmt, StringPart, Val};
use fshell_engine::{
    Env, Flow, PipeStream, PipelinePayload, eval_stmt, execute_pipeline, send_with_backpressure,
};
use fshell_test_support::fixture::ShellFixture;
use std::future::{Future, poll_fn};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::task::{Context, Waker};
use std::time::Duration;
use tokio::sync::{Notify, mpsc};

fn environment() -> Env {
    let env = Env::for_command();
    env.options.write().pipeline_channel_size = 1;
    env.options.write().errexit = false;
    env
}

fn command(name: &str) -> PipelineStage {
    PipelineStage::CommandCall {
        name: name.into(),
        args: Vec::new(),
        env: Vec::new(),
        span: (0, 0).into(),
    }
}

fn data(value: Val) -> PipelinePayload {
    PipelinePayload::Data(Arc::new(value))
}

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("pipeline protocol exceeded deadline")
}

async fn drain(mut rx: PipeStream) -> Vec<PipelinePayload> {
    let mut output = Vec::new();
    while let Some(payload) = rx.recv().await {
        assert!(output.len() < 256, "unexpected unbounded pipeline output");
        output.push(payload);
    }
    output
}

async fn execute(pipeline: &Pipeline, env: &Env) -> Vec<PipelinePayload> {
    let (tx, rx) = mpsc::channel(1);
    let (result, output) =
        bounded(async { tokio::join!(execute_pipeline(pipeline, env, tx), drain(rx)) }).await;
    result.expect("pipeline dispatch and task joins");
    output
}

async fn statement(
    pipeline: Pipeline,
    mut env: Env,
) -> (
    Result<Flow, fshell_engine::EngineError>,
    Vec<PipelinePayload>,
) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    env.output = Some(tx);
    let result = bounded(eval_stmt(
        &Stmt::Expr(Expr::Pipeline(pipeline)),
        &env,
        false,
    ))
    .await;
    drop(env);
    let output = bounded(async {
        let mut output = Vec::new();
        while let Some(payload) = rx.recv().await {
            assert!(output.len() < 256, "unexpected unbounded statement output");
            output.push(payload);
        }
        output
    })
    .await;
    (result, output)
}

fn register_source(env: &Env, payloads: Vec<PipelinePayload>) {
    env.register_async_builtin("source-stage", move |input, args, _env, tx, _span| {
        let payloads = payloads.clone();
        async move {
            assert!(input.is_none());
            assert!(args.is_empty());
            for payload in payloads {
                tx.send(payload).await.expect("source output open");
            }
            Ok(())
        }
    });
}

fn register_forward(env: &Env) {
    env.register_async_builtin("forward-stage", |input, _, _, tx, _| async move {
        let mut input = input.expect("inter-stage input");
        while let Some(payload) = input.recv().await {
            assert!(
                !matches!(payload, PipelinePayload::Structured(_)),
                "diagnostics must bypass data consumers"
            );
            tx.send(payload).await.expect("forward output open");
        }
        Ok(())
    });
}

fn assert_data(payload: &PipelinePayload, expected: &Val) {
    let PipelinePayload::Data(actual) = payload else {
        unreachable!("expected typed value: {payload:?}")
    };
    assert_eq!(actual.as_ref(), expected);
}

fn assert_diagnostic(payload: &PipelinePayload, expected: &ShellError) {
    let PipelinePayload::Structured(actual) = payload else {
        unreachable!("expected diagnostic: {payload:?}")
    };
    assert_eq!(actual.report.downcast_ref::<ShellError>(), Some(expected));
}

#[tokio::test]
async fn multi_stage_transport_preserves_types_binary_chunks_and_data_order() {
    let env = environment();
    register_source(
        &env,
        vec![
            data(Val::Int(7)),
            PipelinePayload::Bytes(vec![0, 255, b'\n'].into()),
            data(Val::List(vec![Val::Bool(false), Val::Int(3)])),
            PipelinePayload::Bytes(vec![b'\r', 0].into()),
        ],
    );
    register_forward(&env);
    let output = execute(
        &Pipeline::new(vec![
            command("source-stage"),
            command("forward-stage"),
            command("forward-stage"),
        ]),
        &env,
    )
    .await;
    assert_eq!(output.len(), 4);
    assert_data(&output[0], &Val::Int(7));
    let PipelinePayload::Bytes(bytes) = &output[1] else {
        unreachable!("expected bytes")
    };
    assert_eq!(bytes.as_ref(), &[0, 255, b'\n']);
    assert_data(&output[2], &Val::List(vec![Val::Bool(false), Val::Int(3)]));
    let PipelinePayload::Bytes(bytes) = &output[3] else {
        unreachable!("expected bytes")
    };
    assert_eq!(bytes.as_ref(), &[b'\r', 0]);
}

#[tokio::test]
async fn diagnostics_bypass_data_consumers_and_keep_the_complete_typed_report() {
    let env = environment();
    let error = ShellError::new(ErrorCode::CommandFailed, "utility failed")
        .with_exit_status(23)
        .with_span((4, 6).into())
        .with_help("help text")
        .with_fix("fix text")
        .with_suggestion("suggestion");
    register_source(
        &env,
        vec![
            data(Val::Int(1)),
            PipelinePayload::Structured(FshDiag::new(error.clone())),
            data(Val::Int(2)),
        ],
    );
    register_forward(&env);
    let output = execute(
        &Pipeline::new(vec![command("source-stage"), command("forward-stage")]),
        &env,
    )
    .await;
    let values: Vec<_> = output
        .iter()
        .filter_map(|payload| {
            if let PipelinePayload::Data(value) = payload {
                Some(value.as_ref().clone())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(values, [Val::Int(1), Val::Int(2)]);
    let diagnostics: Vec<_> = output
        .iter()
        .filter(|payload| matches!(payload, PipelinePayload::Structured(_)))
        .collect();
    assert_eq!(output.len(), 3);
    assert_eq!(diagnostics.len(), 1);
    assert_diagnostic(diagnostics[0], &error);
}

#[tokio::test]
async fn empty_source_closes_every_stage_and_the_output_without_phantom_values() {
    let env = environment();
    register_source(&env, Vec::new());
    register_forward(&env);
    let output = execute(
        &Pipeline::new(vec![
            command("source-stage"),
            command("forward-stage"),
            PipelineStage::Count,
        ]),
        &env,
    )
    .await;
    assert_eq!(output.len(), 1);
    assert_data(&output[0], &Val::Int(0));
}

#[tokio::test]
async fn full_channel_suspends_a_send_until_the_consumer_releases_capacity() {
    let env = environment();
    let (tx, mut rx) = mpsc::channel(1);
    tx.send(data(Val::Int(1))).await.expect("first payload");
    {
        let send = send_with_backpressure(&env, &tx, data(Val::Int(2)));
        tokio::pin!(send);
        assert!(
            send.as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending(),
            "full channel must backpressure, not discard or admit the second payload"
        );
        assert_data(&rx.recv().await.expect("first payload"), &Val::Int(1));
        bounded(send).await.expect("capacity released");
    }
    assert_data(&rx.recv().await.expect("second payload"), &Val::Int(2));
    drop(tx);
    assert!(bounded(rx.recv()).await.is_none());
}

#[tokio::test]
async fn shell_cancellation_wakes_a_backpressured_send_without_delivering_its_payload() {
    let env = environment();
    let (tx, mut rx) = mpsc::channel(1);
    tx.send(data(Val::Int(1))).await.expect("first payload");
    {
        let send = send_with_backpressure(&env, &tx, data(Val::Int(2)));
        tokio::pin!(send);
        assert!(
            send.as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        env.request_shell_cancellation();
        assert_eq!(bounded(send).await, Err(()));
    }
    assert_data(&rx.recv().await.expect("retained payload"), &Val::Int(1));
    drop(tx);
    assert!(bounded(rx.recv()).await.is_none());
}

#[tokio::test]
async fn closing_the_receiver_wakes_a_backpressured_send_and_reports_failure() {
    let env = environment();
    let (tx, rx) = mpsc::channel(1);
    tx.send(data(Val::Int(1))).await.expect("first payload");
    let send = send_with_backpressure(&env, &tx, data(Val::Int(2)));
    tokio::pin!(send);
    assert!(
        send.as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
    drop(rx);
    assert_eq!(bounded(send).await, Err(()));
}

#[tokio::test]
async fn early_limit_closure_stops_the_upstream_producer_and_joins_the_graph() {
    let env = environment();
    let closed = Arc::new(AtomicBool::new(false));
    let observed = closed.clone();
    env.register_async_builtin("source-stage", move |_, _, env, tx, _| {
        let observed = observed.clone();
        async move {
            for value in 0..64 {
                if send_with_backpressure(&env, &tx, data(Val::Int(value)))
                    .await
                    .is_err()
                {
                    observed.store(true, Ordering::Release);
                    return Ok(());
                }
            }
            unreachable!("early consumer must close before the finite overflow budget");
        }
    });
    let output = execute(
        &Pipeline::new(vec![
            command("source-stage"),
            PipelineStage::Limit {
                amount: Expr::Int(1),
            },
        ]),
        &env,
    )
    .await;
    assert_eq!(output.len(), 1);
    assert_data(&output[0], &Val::Int(0));
    assert!(
        closed.load(Ordering::Acquire),
        "completion must include upstream shutdown"
    );
}

#[tokio::test]
async fn hard_failure_cancels_a_producer_proven_to_be_blocked_on_the_real_stage_channel() {
    let env = environment();
    let blocked = Arc::new(Notify::new());
    let stopped = Arc::new(Notify::new());
    let retained_input = Arc::new(tokio::sync::Mutex::new(None));
    let ready = blocked.clone();
    let observed = stopped.clone();
    env.register_async_builtin("source-stage", move |_, _, env, tx, _| {
        let ready = ready.clone();
        let observed = observed.clone();
        async move {
            for value in 0..64 {
                let send = send_with_backpressure(&env, &tx, data(Val::Int(value)));
                tokio::pin!(send);
                let result = poll_fn(|cx| {
                    let result = send.as_mut().poll(cx);
                    if result.is_pending() {
                        ready.notify_one();
                    }
                    result
                })
                .await;
                if result.is_err() {
                    observed.notify_one();
                    return Ok(());
                }
            }
            unreachable!("holding the consumer input must backpressure the source");
        }
    });
    let error = ShellError::new(ErrorCode::DivisionByZero, "hard stage failure");
    let failure = error.clone();
    let lease = retained_input.clone();
    env.register_async_builtin("failure-stage", move |input, _, _, _, _| {
        let blocked = blocked.clone();
        let failure = failure.clone();
        let lease = lease.clone();
        async move {
            blocked.notified().await;
            assert!(input.is_some());
            // Retain the real consumer input until the producer observes stop.
            // Otherwise receiver closure could satisfy the test even if the
            // engine failed to propagate its cancellation signal.
            *lease.lock().await = input;
            Err(failure)
        }
    });
    let pipeline = Pipeline::new(vec![command("source-stage"), command("failure-stage")]);
    let (output, ()) = bounded(async {
        tokio::join!(execute(&pipeline, &env), async {
            stopped.notified().await;
            assert!(retained_input.lock().await.take().is_some());
        })
    })
    .await;
    assert_eq!(output.len(), 1);
    assert_diagnostic(&output[0], &error);
    assert!(
        !env.pipeline_cancelled(),
        "failure stop belongs to this graph, not the shell"
    );
}

#[tokio::test]
async fn pipeline_status_uses_stage_position_not_status_recording_order() {
    for pipefail in [false, true] {
        for first_is_late in [false, true] {
            let env = environment();
            env.set_exit_code(41);
            env.options.write().pipefail = pipefail;
            let recorded = Arc::new(Notify::new());
            let first = recorded.clone();
            env.register_async_builtin("first-stage", move |_, _, env, _, _| {
                let recorded = first.clone();
                async move {
                    if first_is_late {
                        recorded.notified().await;
                    }
                    assert_eq!(env.exit_code(), 41, "in-flight statuses must not publish");
                    env.report_stage_error_code(17);
                    if !first_is_late {
                        recorded.notify_one();
                    }
                    Ok(())
                }
            });
            env.register_async_builtin("last-stage", move |_, _, env, _, _| {
                let recorded = recorded.clone();
                async move {
                    if !first_is_late {
                        recorded.notified().await;
                    }
                    assert_eq!(env.exit_code(), 41);
                    env.report_stage_error_code(0);
                    if first_is_late {
                        recorded.notify_one();
                    }
                    Ok(())
                }
            });
            let (result, output) = statement(
                Pipeline::new(vec![command("first-stage"), command("last-stage")]),
                env.clone(),
            )
            .await;
            assert_eq!(result.expect("ordinary stage statuses"), Flow::Normal);
            assert!(output.is_empty());
            assert_eq!(env.exit_code(), if pipefail { 17 } else { 0 });
        }
    }
}

#[tokio::test]
async fn pipefail_chooses_the_rightmost_nonzero_stage_not_the_largest_status() {
    let env = environment();
    env.options.write().pipefail = true;
    for (name, status) in [("first-stage", 73), ("middle-stage", 17), ("last-stage", 0)] {
        env.register_async_builtin(name, move |_, _, env, _, _| async move {
            env.report_stage_error_code(status);
            Ok(())
        });
    }
    let (result, output) = statement(
        Pipeline::new(vec![
            command("first-stage"),
            command("middle-stage"),
            command("last-stage"),
        ]),
        env.clone(),
    )
    .await;
    assert_eq!(result.expect("nonzero stage status"), Flow::Normal);
    assert!(output.is_empty());
    assert_eq!(env.exit_code(), 17);
}

#[tokio::test]
async fn downstream_success_does_not_turn_a_hard_failure_into_command_success() {
    let env = environment();
    let failure = ShellError::new(ErrorCode::DivisionByZero, "hard stage failure");
    let error = failure.clone();
    env.register_async_builtin("failure-stage", move |_, _, _, _, _| {
        let error = error.clone();
        async move { Err(error) }
    });
    env.register_async_builtin("last-stage", |_, _, _, _, _| async { Ok(()) });
    let (result, output) = statement(
        Pipeline::new(vec![command("failure-stage"), command("last-stage")]),
        env.clone(),
    )
    .await;
    assert!(
        matches!(result, Err(fshell_engine::EngineError::PipelineError { ref message, span: None }) if message == "hard stage failure"),
        "hard errors must unwind: {result:?}"
    );
    assert_eq!(env.exit_code(), 1);
    assert_eq!(output.len(), 1);
    assert_diagnostic(&output[0], &failure);
}

#[tokio::test]
async fn upstream_logical_false_leaves_a_successful_last_stage_successful_without_pipefail() {
    let env = environment();
    env.register_async_builtin("false-stage", |_, _, _, _, _| async {
        Err(ShellError::condition_false())
    });
    env.register_async_builtin("last-stage", |_, _, _, _, _| async { Ok(()) });
    let (result, output) = statement(
        Pipeline::new(vec![command("false-stage"), command("last-stage")]),
        env.clone(),
    )
    .await;
    assert_eq!(env.exit_code(), 0);
    assert_eq!(result.expect("last stage succeeded"), Flow::Normal);
    assert_eq!(output.len(), 1);
    assert_diagnostic(&output[0], &ShellError::condition_false());
}

#[tokio::test]
async fn redirected_diagnostic_text_does_not_erase_the_stage_failure() -> std::io::Result<()> {
    let fixture = ShellFixture::new()?;
    let env = environment();
    env.set_cwd(fixture.workspace());
    let failure = ShellError::new(ErrorCode::CommandFailed, "utility failed").with_exit_status(23);
    let error = failure.clone();
    env.register_async_builtin("failure-stage", move |_, _, _, _, _| {
        let error = error.clone();
        async move { Err(error) }
    });
    let pipeline = Pipeline {
        stages: vec![
            command("failure-stage"),
            PipelineStage::Write {
                path: Expr::String(vec![StringPart::unquoted("diagnostic.txt")]),
                append: false,
                redirect_stdout: false,
                redirect_stderr: true,
            },
        ],
        boundaries: Vec::new(),
    };
    let (result, output) = statement(pipeline, env.clone()).await;
    assert_eq!(result.expect("reported utility failure"), Flow::Normal);
    assert_eq!(env.exit_code(), 23);
    assert!(
        output.is_empty(),
        "redirected diagnostics must not leak to the captured stream"
    );
    assert_eq!(fixture.read_file("diagnostic.txt")?, b"utility failed\n");
    fixture.close()
}

#[tokio::test]
async fn stderr_stream_content_is_not_a_failure_or_a_typed_data_record() {
    let env = environment();
    register_source(
        &env,
        vec![
            PipelinePayload::Structured(FshDiag::new(StderrStream("warning\n".into()))),
            data(Val::Int(7)),
        ],
    );
    register_forward(&env);
    let (result, output) = statement(
        Pipeline::new(vec![command("source-stage"), command("forward-stage")]),
        env.clone(),
    )
    .await;
    assert_eq!(result.expect("stderr is output"), Flow::Normal);
    assert_eq!(env.exit_code(), 0);
    assert_eq!(output.len(), 2);
    let diagnostics: Vec<_> = output
        .iter()
        .filter_map(|payload| {
            if let PipelinePayload::Structured(diag) = payload {
                Some(diag)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].stderr_text(), Some("warning\n".into()));
    let values: Vec<_> = output
        .iter()
        .filter_map(|payload| {
            if let PipelinePayload::Data(value) = payload {
                Some(value.as_ref().clone())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(values, [Val::Int(7)]);
}

#[tokio::test]
async fn successful_last_stage_allows_the_following_and_branch_to_execute() {
    let env = environment();
    env.vars.write().insert("followed".into(), Val::Bool(false));
    env.register_async_builtin("false-stage", |_, _, _, _, _| async {
        Err(ShellError::condition_false())
    });
    env.register_async_builtin("last-stage", |_, _, _, _, _| async { Ok(()) });
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut captured = env.clone();
    captured.output = Some(tx);
    let stmt = Stmt::And(
        Box::new(Stmt::Expr(Expr::Pipeline(Pipeline::new(vec![
            command("false-stage"),
            command("last-stage"),
        ])))),
        Box::new(Stmt::Assign {
            name: "followed".into(),
            expr: Expr::Bool(true),
        }),
    );
    assert_eq!(
        bounded(eval_stmt(&stmt, &captured, false))
            .await
            .expect("and chain"),
        Flow::Normal
    );
    assert_eq!(env.vars.read().get("followed"), Some(&Val::Bool(true)));
    assert_eq!(env.exit_code(), 0);
    drop(captured);
    let output = bounded(async {
        let mut output = Vec::new();
        while let Some(payload) = rx.recv().await {
            assert!(output.len() < 2);
            output.push(payload);
        }
        output
    })
    .await;
    assert_eq!(output.len(), 1);
    assert_diagnostic(&output[0], &ShellError::condition_false());
}

#[tokio::test]
async fn reported_failure_keeps_its_status_and_diagnostic_without_aborting_later_stages() {
    for pipefail in [false, true] {
        let env = environment();
        env.options.write().pipefail = pipefail;
        let error =
            ShellError::new(ErrorCode::CommandFailed, "utility failed").with_exit_status(23);
        let reported = error.clone();
        env.register_async_builtin("failure-stage", move |_, _, _, _, _| {
            let reported = reported.clone();
            async move { Err(reported) }
        });
        let ran = Arc::new(AtomicBool::new(false));
        let observed = ran.clone();
        env.register_async_builtin("last-stage", move |_, _, _, _, _| {
            let observed = observed.clone();
            async move {
                observed.store(true, Ordering::Release);
                Ok(())
            }
        });
        let (result, output) = statement(
            Pipeline::new(vec![command("failure-stage"), command("last-stage")]),
            env.clone(),
        )
        .await;
        assert_eq!(result.expect("reported failure"), Flow::Normal);
        assert_eq!(env.exit_code(), if pipefail { 23 } else { 0 });
        assert!(ran.load(Ordering::Acquire));
        assert_eq!(output.len(), 1);
        assert_diagnostic(&output[0], &error);
    }
}

#[tokio::test]
async fn hard_failure_wakes_waiting_stages_without_cancelling_the_shell_or_next_pipeline() {
    let env = environment();
    let source_ready = Arc::new(Notify::new());
    let consumer_ready = Arc::new(Notify::new());
    let source_stopped = Arc::new(AtomicBool::new(false));
    let consumer_stopped = Arc::new(AtomicBool::new(false));
    let ready = source_ready.clone();
    let stopped = source_stopped.clone();
    env.register_async_builtin("source-stage", move |_, _, env, _, _| {
        let ready = ready.clone();
        let stopped = stopped.clone();
        async move {
            ready.notify_one();
            env.wait_pipeline_cancelled().await;
            assert!(env.pipeline_cancelled());
            stopped.store(true, Ordering::Release);
            Ok(())
        }
    });
    let ready = consumer_ready.clone();
    let stopped = consumer_stopped.clone();
    env.register_async_builtin("last-stage", move |input, _, env, _, _| {
        let ready = ready.clone();
        let stopped = stopped.clone();
        async move {
            ready.notify_one();
            env.wait_pipeline_cancelled().await;
            assert!(env.pipeline_cancelled());
            assert!(input.is_some());
            drop(input);
            stopped.store(true, Ordering::Release);
            Ok(())
        }
    });
    let failure = ShellError::new(ErrorCode::DivisionByZero, "hard stage failure");
    let error = failure.clone();
    env.register_async_builtin("failure-stage", move |_, _, _, _, _| {
        let source_ready = source_ready.clone();
        let consumer_ready = consumer_ready.clone();
        let error = error.clone();
        async move {
            source_ready.notified().await;
            consumer_ready.notified().await;
            Err(error)
        }
    });
    let output = execute(
        &Pipeline::new(vec![
            command("source-stage"),
            command("failure-stage"),
            command("last-stage"),
        ]),
        &env,
    )
    .await;
    assert_eq!(output.len(), 1);
    assert_diagnostic(&output[0], &failure);
    assert!(source_stopped.load(Ordering::Acquire));
    assert!(consumer_stopped.load(Ordering::Acquire));
    assert!(!env.pipeline_cancelled());
    register_source(&env, vec![data(Val::Int(8))]);
    let next = execute(&Pipeline::new(vec![command("source-stage")]), &env).await;
    assert_eq!(next.len(), 1);
    assert_data(&next[0], &Val::Int(8));
}

#[tokio::test]
async fn intrinsic_filter_and_map_shadow_record_fields_but_retain_enclosing_bindings() {
    let env = environment().push_scope(Arc::new(fshell_core::RwLock::new(Default::default())));
    let locals = env.local_vars.as_ref().expect("enclosing scope");
    locals.declare("field", Val::Int(99));
    locals.declare("offset", Val::Int(10));
    let record = |value| Val::Map([("field".into(), Val::Int(value))].into_iter().collect());
    register_source(&env, vec![data(record(1)), data(record(2))]);
    let filter = PipelineStage::Filter {
        condition: Expr::BinaryOp {
            op: fshell_core::BinOp::Gt,
            lhs: Box::new(Expr::Variable("field".into())),
            rhs: Box::new(Expr::Int(1)),
        },
    };
    let map = PipelineStage::Map {
        projections: vec![
            Expr::Variable("field".into()),
            Expr::BinaryOp {
                op: fshell_core::BinOp::Add,
                lhs: Box::new(Expr::Variable("field".into())),
                rhs: Box::new(Expr::Variable("offset".into())),
            },
        ],
    };
    let output = execute(
        &Pipeline::new(vec![command("source-stage"), filter, map]),
        &env,
    )
    .await;
    assert_eq!(output.len(), 1);
    assert_data(
        &output[0],
        &Val::Map(
            [
                ("field".into(), Val::Int(2)),
                ("col_1".into(), Val::Int(12)),
            ]
            .into_iter()
            .collect(),
        ),
    );
    assert_eq!(locals.get("field"), Some(Val::Int(99)));
    assert_eq!(locals.get("offset"), Some(Val::Int(10)));
    assert!(!env.vars.read().contains_key("field"));
}

#[tokio::test]
async fn intrinsic_expression_failures_keep_error_type_and_original_source_span() {
    let missing = Expr::Spanned {
        expr: Box::new(Expr::Variable("missing_field".into())),
        span: (7, 13).into(),
    };
    for (stage, input) in [
        (
            PipelineStage::Filter {
                condition: missing.clone(),
            },
            data(Val::Int(1)),
        ),
        (
            PipelineStage::Filter {
                condition: missing.clone(),
            },
            PipelinePayload::Bytes(b"line\n".to_vec().into()),
        ),
        (
            PipelineStage::Map {
                projections: vec![missing.clone()],
            },
            data(Val::Map(Default::default())),
        ),
    ] {
        let env = environment();
        register_source(&env, vec![input]);
        let (result, output) = statement(
            Pipeline::new(vec![command("source-stage"), stage]),
            env.clone(),
        )
        .await;
        assert!(
            matches!(result, Err(fshell_engine::EngineError::VariableNotFound { ref name, span: Some(span) }) if name == "missing_field" && span == (7, 13).into()),
            "lost typed error: {result:?}"
        );
        assert_eq!(env.exit_code(), 1);
        assert_eq!(output.len(), 1);
        let PipelinePayload::Structured(diag) = &output[0] else {
            unreachable!("expected diagnostic")
        };
        assert!(
            matches!(diag.report.downcast_ref::<fshell_engine::EngineError>(), Some(fshell_engine::EngineError::VariableNotFound { name, span: Some(span) }) if name == "missing_field" && *span == (7, 13).into())
        );
    }
}

#[tokio::test]
async fn filter_skips_a_record_missing_its_column_but_processes_the_following_record() {
    let env = environment();
    let record = Val::Map([("field".into(), Val::Int(7))].into_iter().collect());
    register_source(
        &env,
        vec![data(Val::Map(Default::default())), data(record.clone())],
    );
    let filter = PipelineStage::Filter {
        condition: Expr::BinaryOp {
            op: fshell_core::BinOp::Gt,
            lhs: Box::new(Expr::Variable("field".into())),
            rhs: Box::new(Expr::Int(1)),
        },
    };
    let output = execute(&Pipeline::new(vec![command("source-stage"), filter]), &env).await;
    assert_eq!(output.len(), 1);
    assert_data(&output[0], &record);
}

#[tokio::test]
async fn nonboolean_filter_predicates_are_typed_failures_not_silent_drops() {
    for input in [
        data(Val::Int(7)),
        PipelinePayload::Bytes(b"line\n".to_vec().into()),
    ] {
        let env = environment();
        register_source(&env, vec![input]);
        let (result, output) = statement(
            Pipeline::new(vec![
                command("source-stage"),
                PipelineStage::Filter {
                    condition: Expr::Int(3),
                },
            ]),
            env.clone(),
        )
        .await;
        let expected = ShellError::new(
            ErrorCode::TypeError,
            "filter predicate must evaluate to Bool, got Int",
        );
        assert!(
            matches!(result, Err(fshell_engine::EngineError::PipelineError { ref message, span: None }) if message == &expected.message),
            "{result:?}"
        );
        assert_eq!(env.exit_code(), 1);
        assert_eq!(output.len(), 1);
        assert_diagnostic(&output[0], &expected);
    }
}
