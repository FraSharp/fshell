mod common;
use common::*;
use fshell_engine::PipelinePayload;
use std::sync::Arc;

async fn filter_payloads(payloads: Vec<PipelinePayload>, args: &[&str]) -> Vec<Val> {
    let env = setup_test_env();
    let (input_tx, input_rx) = tokio::sync::mpsc::channel(2);
    let (output_tx, mut output_rx) = tokio::sync::mpsc::channel(2);
    let arguments = args.iter().map(|s| Val::String((*s).into())).collect();
    let producer = tokio::spawn(async move {
        for payload in payloads {
            if input_tx.send(payload).await.is_err() {
                break;
            }
        }
    });
    let search = tokio::task::spawn_blocking(move || {
        fshell_builtins::grep::grep_builtin(Some(input_rx), arguments, &env, output_tx, None)
            .map_err(Box::new)
    });
    let mut output = vec![];
    while let Some(payload) = output_rx.recv().await {
        match payload {
            PipelinePayload::Data(value) => output.push((*value).clone()),
            PipelinePayload::Structured(diag) => panic!("{diag:?}"),
            _ => panic!("unexpected raw output"),
        }
    }
    let result = search.await.unwrap();
    assert!(result.is_ok() || result.is_err_and(|e| e.is_condition_false()));
    producer.await.unwrap();
    output
}

#[tokio::test]
async fn byte_streams_preserve_utf8_at_every_chunk_boundary() {
    let bytes = "before\nαβ\nafter\n終".as_bytes();
    for size in 1..=bytes.len() {
        let payloads = bytes
            .chunks(size)
            .map(|b| PipelinePayload::Bytes(b.to_vec().into()))
            .collect();
        assert_eq!(
            filter_payloads(payloads, &["αβ|終"]).await,
            [Val::String("αβ".into()), Val::String("終".into())]
        );
    }
}

#[tokio::test]
async fn invalid_utf8_matches_keep_their_bytes() {
    let payloads = vec![PipelinePayload::Bytes(b"prefix\xff\n".to_vec().into())];
    assert_eq!(
        filter_payloads(payloads, &["-F", "prefix"]).await,
        [Val::Blob(b"prefix\xff".to_vec())]
    );
}

#[tokio::test]
async fn structured_selection_matches_values_not_keys_and_preserves_records() {
    let mut map = fshell_core::FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    map.insert(ustr::ustr("needle"), Val::String("haystack".into()));
    let value = Val::Map(map);
    let payloads = || vec![PipelinePayload::Data(Arc::new(value.clone()))];
    assert!(filter_payloads(payloads(), &["needle"]).await.is_empty());
    assert_eq!(filter_payloads(payloads(), &["^haystack$"]).await, [value]);
}

#[tokio::test]
async fn count_scanners_leave_typed_records_for_the_record_adapter() {
    let mut map = fshell_core::FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
    map.insert(ustr::ustr("value"), Val::String("hit".into()));
    let payloads = || {
        vec![
            PipelinePayload::Bytes(b"hit\n".to_vec().into()),
            PipelinePayload::Data(Arc::new(Val::Map(map.clone()))),
            PipelinePayload::Bytes(b"miss\nhit\n".to_vec().into()),
        ]
    };
    assert_eq!(
        filter_payloads(payloads(), &["-c", "hit"]).await,
        [Val::Int(3)]
    );
    assert_eq!(
        filter_payloads(payloads(), &["-cm2", "hit"]).await,
        [Val::Int(2)]
    );
}

#[tokio::test]
async fn quiet_closes_the_input_and_count_emits_only_the_total() {
    let payloads = || vec![PipelinePayload::Bytes(b"hit\nhit\nmiss\n".to_vec().into())];
    assert!(filter_payloads(payloads(), &["-q", "hit"]).await.is_empty());
    assert_eq!(
        filter_payloads(payloads(), &["-c", "hit"]).await,
        [Val::Int(2)]
    );
}
