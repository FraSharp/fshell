//! Native typed pipelines exercise registered builtin handlers and collection.

use fshell_core::{Expr, Parser, Stmt, Val};
use fshell_engine::{Env, collect_pipeline};

async fn collect(source: &str, env: &Env) -> Vec<Val> {
    let stmts = Parser::new(source)
        .parse_statements()
        .expect("valid native pipeline");
    assert_eq!(stmts.len(), 1);
    let Stmt::Expr(expr) = stmts[0].unpack() else {
        unreachable!("pipeline fixture must be an expression")
    };
    let Expr::Pipeline(pipeline) = expr.unpack() else {
        unreachable!("pipeline fixture must parse as a pipeline")
    };
    collect_pipeline(pipeline, env)
        .await
        .expect("typed pipeline collection")
}

#[tokio::test]
async fn sorting_preserves_every_integer_including_duplicates() {
    let env = Env::for_command();
    fshell_builtins::init(&env);
    let input = vec![Val::Int(3), Val::Int(-2), Val::Int(3), Val::Int(0)];
    env.vars
        .write()
        .insert("items".into(), Val::List(input.clone()));
    assert_eq!(
        collect("$items | sort", &env).await,
        vec![Val::Int(-2), Val::Int(0), Val::Int(3), Val::Int(3)]
    );
    assert_eq!(
        env.vars.read().get("items"),
        Some(&Val::List(input)),
        "pipeline must not mutate its input binding"
    );
}

#[tokio::test]
async fn empty_typed_input_has_zero_count_without_a_phantom_record() {
    let env = Env::for_command();
    fshell_builtins::init(&env);
    env.vars
        .write()
        .insert("items".into(), Val::List(Vec::new()));
    assert_eq!(collect("$items | count", &env).await, vec![Val::Int(0)]);
    assert_eq!(collect("$items | sort", &env).await, Vec::<Val>::new());
}

#[tokio::test]
async fn limit_is_a_prefix_operation_and_count_observes_only_that_prefix() {
    let env = Env::for_command();
    fshell_builtins::init(&env);
    env.vars.write().insert(
        "items".into(),
        Val::List(vec![Val::Int(5), Val::Int(2), Val::Int(9)]),
    );
    assert_eq!(
        collect("$items | limit 2", &env).await,
        vec![Val::Int(5), Val::Int(2)]
    );
    assert_eq!(
        collect("$items | limit 2 | count", &env).await,
        vec![Val::Int(2)]
    );
}
