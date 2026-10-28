// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use fshell_core::ShellError;
use fshell_core::Val;
use fshell_core::diagnostic::ErrorCode;
use fshell_engine::descriptors::Descriptor;
use fshell_engine::read_input::{
    DescriptorLines, LineSource, LogicalLine, assign_read_line, read_logical_line,
};
use fshell_engine::{Env, PipeSender, PipeStream, PipelinePayload};
use fshell_terminal::input::{InputEvent, InputPoll, Key, UnixEventSource};
use miette::SourceSpan;
use std::time::Duration;
use tokio::time::timeout;

/// `read [-r] [-s] [-p prompt] [-t seconds] [name ...]`: read one logical line
/// and assign it like the POSIX utility (`$IFS` fields, `REPLY` by default).
/// It outputs nothing; its answer is its status, 1 at end of input or on a
/// timeout.
pub fn read_builtin(
    in_rx: Option<PipeStream>,
    args: Vec<Val>,
    env: &Env,
    tx: PipeSender,
    span: Option<SourceSpan>,
) -> Result<(), ShellError> {
    let mut prompt = None;
    let mut timeout_secs = None;
    let mut silent = false;
    let mut raw = false;
    let mut names = Vec::new();

    let usage = |message: &str| {
        ShellError::new(ErrorCode::InvalidArgument, format!("read: {message}"))
            .maybe_with_span(span)
    };
    let mut words = args.iter().map(Val::to_text);
    while let Some(word) = words.next() {
        match word.as_str() {
            "-r" => raw = true,
            "-s" => silent = true,
            "-p" => prompt = Some(words.next().ok_or_else(|| usage("-p needs a prompt"))?),
            "-t" => {
                let value = words.next().ok_or_else(|| usage("-t needs a timeout"))?;
                timeout_secs = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| usage("invalid timeout value"))?,
                );
            }
            "--" => {
                names.extend(words.by_ref());
                break;
            }
            option if option.starts_with('-') && option.len() > 1 => {
                return Err(usage(&format!("{option}: invalid option")));
            }
            name => names.push(name.to_string()),
        }
    }

    let env = env.clone();
    tokio::spawn(async move {
        // The stage lasts until the line is assigned: holding the (unused)
        // output sender is what keeps the statement from finishing first.
        let _stage_open = tx;
        let reading = async {
            match in_rx {
                Some(rx) => read_logical_line(&mut ChannelLines::new(rx), raw).await,
                None if silent && unsafe { libc::isatty(libc::STDIN_FILENO) } == 1 => {
                    let deadline = timeout_secs
                        .map(|secs| std::time::Instant::now() + Duration::from_secs(secs));
                    tokio::task::spawn_blocking(move || read_line_silent(deadline))
                        .await
                        .map_err(std::io::Error::other)?
                        .map(|text| LogicalLine {
                            text,
                            terminated: true,
                        })
                }
                None => {
                    // A prompt is for a person at a terminal, on stderr as in
                    // other shells, so it never mixes into captured output.
                    if let Some(prompt) = &prompt
                        && unsafe { libc::isatty(libc::STDIN_FILENO) } == 1
                    {
                        eprint!("{prompt}");
                    }
                    let stdin = Descriptor::Process(libc::STDIN_FILENO);
                    read_logical_line(&mut DescriptorLines(&stdin), raw).await
                }
            }
        };
        let line = match timeout_secs {
            Some(secs) => match timeout(Duration::from_secs(secs), reading).await {
                Ok(line) => line,
                Err(_) => Ok(LogicalLine {
                    text: String::new(),
                    terminated: false,
                }),
            },
            None => reading.await,
        };
        match line {
            Ok(line) => {
                if assign_read_line(&env, &names, line) != 0 {
                    env.record_command_failure(ShellError::condition_false());
                }
            }
            Err(error) => {
                env.record_command_failure(ShellError::new(
                    ErrorCode::IoError,
                    format!("read: {error}"),
                ));
            }
        }
    });

    Ok(())
}

/// Lines assembled from a pipeline's payloads: a record is one line, bytes
/// are split at newlines.
struct ChannelLines {
    rx: PipeStream,
    pending: Vec<u8>,
    closed: bool,
}

impl ChannelLines {
    fn new(rx: PipeStream) -> Self {
        Self {
            rx,
            pending: Vec::new(),
            closed: false,
        }
    }
}

impl LineSource for ChannelLines {
    async fn next_line(&mut self) -> std::io::Result<Option<Vec<u8>>> {
        loop {
            if let Some(end) = self.pending.iter().position(|&b| b == b'\n') {
                return Ok(Some(self.pending.drain(..=end).collect()));
            }
            if self.closed {
                return Ok((!self.pending.is_empty()).then(|| std::mem::take(&mut self.pending)));
            }
            match self.rx.recv().await {
                Some(PipelinePayload::Data(value)) => {
                    self.pending.extend_from_slice(value.to_text().as_bytes());
                    self.pending.push(b'\n');
                }
                Some(PipelinePayload::Bytes(bytes)) => self.pending.extend_from_slice(&bytes),
                Some(PipelinePayload::Structured(_)) => {}
                None => self.closed = true,
            }
        }
    }
}

/// Read a line from the terminal without echoing it (`read -s`), giving up
/// at `deadline`. Blocking: run it on the blocking pool.
fn read_line_silent(deadline: Option<std::time::Instant>) -> std::io::Result<String> {
    fshell_terminal::enable_raw_mode()?;
    let mut line = String::new();
    let mut input = UnixEventSource::new();
    let result = loop {
        if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
            break Ok(String::new());
        }
        let key = match input.poll(Duration::from_millis(100)) {
            Ok(InputPoll::Event(InputEvent::Key(key))) => key,
            Ok(InputPoll::Event(_) | InputPoll::Timeout) => continue,
            Ok(InputPoll::Closed) => {
                break Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
            }
            Err(error) => break Err(std::io::Error::other(error)),
        };
        match key.key {
            Key::Enter => break Ok(line),
            Key::Escape => break Ok(String::new()),
            Key::Backspace => {
                line.pop();
            }
            Key::Character(c) => line.push(c),
            _ => {}
        }
    };
    let _ = fshell_terminal::disable_raw_mode();
    // The user's Enter was not echoed; end the line on the terminal.
    eprintln!();
    result
}
