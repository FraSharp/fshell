//! Byte framing and scanning use the shell-independent search implementation.

use fshell_search::Control;
use fshell_search::content::{CompiledMatcher, MatchOptions, ScanEvent, ScanOptions, Scanner};
use fshell_search::framing::LineFramer;
use std::io;

#[test]
fn every_two_chunk_partition_preserves_line_bytes_and_utf8_boundaries() {
    let input = "αβ\n\nhello\n終".as_bytes();
    let expected: Vec<Vec<u8>> = ["αβ\n", "\n", "hello\n", "終"]
        .iter()
        .map(|line| line.as_bytes().to_vec())
        .collect();
    for split in 0..=input.len() {
        let mut framer = LineFramer::default();
        let mut lines = Vec::new();
        for chunk in [&input[..split], &input[split..]] {
            assert!(framer.push(chunk, |line| {
                lines.push(line.to_vec());
                true
            }));
        }
        assert!(framer.finish(|line| {
            lines.push(line.to_vec());
            true
        }));
        assert_eq!(lines, expected, "split at byte {split}");
    }
}

#[test]
fn framing_emits_the_unterminated_tail_once_and_no_phantom_empty_line() {
    let mut framer = LineFramer::default();
    let mut lines = Vec::new();
    assert!(framer.push(b"first\nlast", |line| {
        lines.push(line.to_vec());
        true
    }));
    assert!(framer.finish(|line| {
        lines.push(line.to_vec());
        true
    }));
    assert!(framer.finish(|line| {
        lines.push(line.to_vec());
        true
    }));
    assert_eq!(lines, [b"first\n".to_vec(), b"last".to_vec()]);
}

#[test]
fn scan_reports_complete_matched_lines_with_positions_and_offsets() -> io::Result<()> {
    let matcher = CompiledMatcher::compile(&["alpha".into()], &MatchOptions::default())
        .expect("valid matcher");
    let mut scanner = Scanner::new(ScanOptions::default()).expect("valid scanner");
    let mut lines = Vec::new();
    let mut breaks = 0;
    let summary = scanner.search(
        &matcher,
        &b"alpha\nother\nalpha tail"[..],
        &Control::default(),
        || false,
        |event| {
            match event {
                ScanEvent::Line {
                    bytes,
                    line,
                    offset,
                    matched,
                } => lines.push((bytes.to_vec(), line, offset, matched)),
                ScanEvent::ContextBreak => breaks += 1,
            }
            Ok(true)
        },
    )?;
    assert_eq!(summary.matches, 2);
    assert_eq!(summary.binary_offset, None);
    assert_eq!(
        lines,
        [
            (b"alpha\n".to_vec(), 1, 0, true),
            (b"alpha tail".to_vec(), 3, 12, true)
        ]
    );
    assert_eq!(breaks, 0);
    Ok(())
}

#[test]
fn cancellation_before_scan_does_not_read_or_emit() -> io::Result<()> {
    struct MustNotRead;
    impl io::Read for MustNotRead {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("cancelled scanner attempted input"))
        }
    }
    let matcher = CompiledMatcher::compile(&["alpha".into()], &MatchOptions::default())
        .expect("valid matcher");
    let mut scanner = Scanner::new(ScanOptions::default()).expect("valid scanner");
    let mut emitted = 0;
    let summary = scanner.search(
        &matcher,
        MustNotRead,
        &Control::default(),
        || true,
        |_| {
            emitted += 1;
            Ok(true)
        },
    )?;
    assert_eq!(summary.matches, 0);
    assert_eq!(emitted, 0);
    Ok(())
}

#[test]
fn sink_failures_propagate_instead_of_becoming_successful_early_termination() {
    let matcher = CompiledMatcher::compile(&["alpha".into()], &MatchOptions::default())
        .expect("valid matcher");
    let mut scanner = Scanner::new(ScanOptions::default()).expect("valid scanner");
    let error = scanner
        .search(
            &matcher,
            &b"alpha\n"[..],
            &Control::default(),
            || false,
            |_| {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected sink rejection",
                ))
            },
        )
        .expect_err("sink failure must propagate");
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
}
