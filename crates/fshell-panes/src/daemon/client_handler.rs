// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Per-client connection handler.
//!
//! Each attached client gets a dedicated handler task that:
//! 1. Reads frames from the client socket (input, resize, commands).
//! 2. Dispatches commands to the session manager.
//! 3. Renders session state at 60 FPS and sends frames to the client.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::net::UnixStream;
use tokio::sync::{RwLock, mpsc};
use tokio_util::codec::Framed;

use crate::layout::bsp::Split;
use crate::proto::Frame;
use crate::proto::codec::FshCodec;
use crate::proto::message::{ClientMessage, PrefixCommand, ServerMessage};

use super::renderer::FrameRenderer;
use super::session::Session;
use super::session_manager::SessionManager;

/// Handle a single client connection.
///
/// This is spawned as a tokio task for each connecting client.
pub async fn handle_client(
    stream: UnixStream,
    session_manager: Arc<RwLock<SessionManager>>,
    daemon_event_tx: mpsc::Sender<super::DaemonEvent>,
) {
    let framed = Framed::new(stream, FshCodec);
    let (mut sink, mut stream) = framed.split();

    // Currently attached session (if any).
    let mut attached_session: Option<Arc<RwLock<Session>>> = None;
    let mut session_name: Option<String> = None;
    // Current terminal dimensions.
    let mut cols: u16 = 80;
    let mut rows: u16 = 24;

    // Persistent renderer — created on first attach, recreated on resize.
    let mut renderer: Option<FrameRenderer> = None;
    let mut last_rendered_time: Option<String> = None;

    // Render tick: 60 FPS — highly responsive with event-driven rendering.
    let mut render_tick = tokio::time::interval(Duration::from_millis(16));
    render_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            // Incoming frame from client.
            frame = stream.next() => {
                match frame {
                    Some(Ok(frame)) => {
                        let msg = match frame.into_client() {
                            Ok(m) => m,
                            Err(e) => {
                                eprintln!("client message decode error: {}", e);
                                continue;
                            }
                        };

                        match msg {
                            ClientMessage::Attach { session_name: name, cols: c, rows: r } => {
                                cols = c;
                                rows = r;

                                // Detach from current session if attached.
                                if attached_session.is_some() {
                                    let mgr = session_manager.read().await;
                                    if let Some(ref n) = session_name {
                                        let _ = mgr.detach_session(n).await;
                                    }
                                    drop(mgr);
                                    attached_session = None;
                                    session_name = None;
                                }

                                // Try to attach, or create the session if it doesn't exist.
                                let needs_create = {
                                    let mgr = session_manager.read().await;
                                    mgr.get_session(&name).await.is_none()
                                };

                                if needs_create {
                                    eprintln!("fshell-panesd: creating session '{}'", name);
                                    let mut mgr = session_manager.write().await;
                                    if let Err(e) = mgr.create_session(name.clone(), cols, rows, daemon_event_tx.clone()).await {
                                        eprintln!("fshell-panesd: failed to create session: {}", e);
                                        continue;
                                    }
                                }

                                // Get the session handle.
                                let sess = {
                                    let mgr = session_manager.read().await;
                                    match mgr.get_session(&name).await {
                                        Some(s) => s,
                                        None => {
                                            eprintln!("fshell-panesd: session '{}' not found after creation", name);
                                            continue;
                                        }
                                    }
                                };

                                attached_session = Some(sess.clone());
                                session_name = Some(name);

                                // Create the renderer for this session.
                                match FrameRenderer::new(cols, rows) {
                                    Ok(r) => renderer = Some(r),
                                    Err(e) => {
                                        eprintln!("fshell-panesd: failed to create renderer: {}", e);
                                        continue;
                                    }
                                }

                                // Send initial render.
                                if let Some(ref mut r) = renderer {
                                    let s = sess.read().await;
                                    if let Some(window) = s.active_window() {
                                        let frame_data = r.render_frame(&s, window, cols, rows);
                                        drop(s);
                                        if let Ok(bytes) = frame_data {
                                            let msg = ServerMessage::Draw(bytes);
                                            if let Err(e) = sink.send(Frame::from_server(&msg)).await {
                                                eprintln!("fshell-panesd: initial render send error: {}", e);
                                                break;
                                            }
                                        }
                                    } else {
                                        drop(s);
                                    }
                                }
                            }

                            ClientMessage::Detach => {
                                attached_session = None;
                                let _ = sink.send(Frame::from_server(&ServerMessage::ExitClient)).await;
                                break;
                            }

                            ClientMessage::Resize { cols: c, rows: r } => {
                                cols = c;
                                rows = r;
                                // Resize the renderer.
                                if let Some(ref mut r) = renderer {
                                    let _ = r.resize(cols, rows);
                                }
                                if let Some(ref sess) = attached_session {
                                    let mut s = sess.write().await;
                                    s.resize_all(cols, rows);
                                }
                            }

                            ClientMessage::Input(bytes) => {
                                if let Some(ref sess) = attached_session {
                                    let s = sess.read().await;
                                    if let Some(window) = s.active_window()
                                        && let Some(pane) = window.panes.get(&window.focus.focused_pane)
                                    {
                                        if let Ok(mut grid) = pane.grid.try_write()
                                            && !grid.is_at_bottom()
                                        {
                                            grid.scroll_to_bottom();
                                        }
                                        let _ = pane.pty_tx.try_send(
                                            crate::pty::grid_manager::PtyCommand::Data(bytes),
                                        );
                                    }
                                }
                            }

                            ClientMessage::PrefixCommand(cmd) => {
                                if let Some(ref sess) = attached_session {
                                    let mut s = sess.write().await;
                                    let ai = s.active_window;
                                    // Exit prefix mode on the active window.
                                    if let Some(window) = s.active_window_mut() {
                                        window.focus.prefix_active = false;
                                    }
                                    match cmd {
                                        PrefixCommand::SplitHorizontal => {
                                            s.spawn_pane(Split::Horizontal);
                                        }
                                        PrefixCommand::SplitVertical => {
                                            s.spawn_pane(Split::Vertical);
                                        }
                                        PrefixCommand::KillPane => {
                                            s.close_pane();
                                            if s.is_empty() || s.windows.is_empty() {
                                                let sess_name = session_name.clone().unwrap_or_default();
                                                drop(s);
                                                let mut mgr = session_manager.write().await;
                                                let _ = mgr.kill_session(&sess_name).await;
                                                let _ = sink.send(Frame::from_server(&ServerMessage::ExitClient)).await;
                                                break;
                                            }
                                        }
                                        PrefixCommand::FocusUp => {
                                            resolve_focus_in_direction(&mut s, ai, Direction::Up);
                                        }
                                        PrefixCommand::FocusDown => {
                                            resolve_focus_in_direction(&mut s, ai, Direction::Down);
                                        }
                                        PrefixCommand::FocusLeft => {
                                            resolve_focus_in_direction(&mut s, ai, Direction::Left);
                                        }
                                        PrefixCommand::FocusRight => {
                                            resolve_focus_in_direction(&mut s, ai, Direction::Right);
                                        }
                                        PrefixCommand::ShowHelp => {
                                            if let Some(w) = s.active_window_mut() {
                                                w.show_help = !w.show_help;
                                                w.help_scroll = 0;
                                            }
                                        }
                                        PrefixCommand::Quit => {
                                            drop(s);
                                            let _ = sink.send(Frame::from_server(&ServerMessage::ExitClient)).await;
                                            let _ = daemon_event_tx.send(
                                                super::DaemonEvent::Shutdown,
                                            ).await;
                                            break;
                                        }
                                        PrefixCommand::WindowNew => {
                                            s.new_window();
                                        }
                                        PrefixCommand::WindowNext => {
                                            s.next_window();
                                        }
                                        PrefixCommand::WindowPrevious => {
                                            s.previous_window();
                                        }
                                        PrefixCommand::WindowSwitch(index) => {
                                            s.switch_window(index as usize);
                                        }
                                        PrefixCommand::WindowClose => {
                                            let is_empty = s.close_active_window();
                                            if is_empty {
                                                let sess_name = session_name.clone().unwrap_or_default();
                                                drop(s);
                                                let mut mgr = session_manager.write().await;
                                                let _ = mgr.kill_session(&sess_name).await;
                                                let _ = sink.send(Frame::from_server(&ServerMessage::ExitClient)).await;
                                                break;
                                            }
                                        }
                                        PrefixCommand::WindowRename => {
                                            if let Some(w) = s.active_window_mut() {
                                                let current = w.name.clone();
                                                w.rename_state = Some(crate::daemon::window::RenameState {
                                                    target: crate::daemon::window::RenameTarget::Window,
                                                    buffer: current,
                                                });
                                            }
                                        }
                                    }
                                }
                            }

                            ClientMessage::PrefixToggle { active } => {
                                if let Some(ref sess) = attached_session {
                                    let mut s = sess.write().await;
                                    if let Some(w) = s.active_window_mut() {
                                        w.focus.prefix_active = active;
                                    }
                                }
                            }

                            ClientMessage::Scroll { lines } => {
                                if let Some(ref sess) = attached_session {
                                    let s = sess.read().await;
                                    if let Some(window) = s.active_window()
                                        && let Some(pane) = window.panes.get(&window.focus.focused_pane)
                                        && let Ok(mut grid) = pane.grid.try_write()
                                    {
                                        if lines > 0 {
                                            grid.scroll_down_viewport(lines as usize);
                                        } else if lines < 0 {
                                            grid.scroll_up_viewport((-lines) as usize);
                                        }
                                    }
                                }
                            }

                            ClientMessage::MouseClick { col, row } => {
                                if let Some(ref sess) = attached_session {
                                    let mut s = sess.write().await;
                                    let term_size = s.terminal_size;
                                    let pane_rows = term_size.1.saturating_sub(1);
                                    if let Some(window) = s.active_window_mut() {
                                        let layout = window.bsp.compute_layout(
                                            ratatui::layout::Rect::new(0, 0, term_size.0, pane_rows),
                                        );
                                        for (pane_id, rect) in &layout {
                                            if col >= rect.x && col < rect.x + rect.width
                                                && row >= rect.y && row < rect.y + rect.height
                                            {
                                                window.focus.set_focused_pane(*pane_id);
                                                break;
                                            }
                                        }
                                    }
                                }
                            }

                            ClientMessage::ListSessions => {
                                let mgr = session_manager.read().await;
                                let list = mgr.list_sessions().await;
                                let _ = sink
                                    .send(Frame::from_server(&ServerMessage::SessionList(list)))
                                    .await;
                            }

                            ClientMessage::KillSession { session_name: name } => {
                                let mut mgr = session_manager.write().await;
                                let _ = mgr.kill_session(&name).await;
                                let _ = sink.send(Frame::from_server(&ServerMessage::Ack)).await;
                            }

                            ClientMessage::HelpKey(_key) => {
                                // When help is open, Esc/q/? toggles it off.
                                if let Some(ref sess) = attached_session {
                                    let mut s = sess.write().await;
                                    if let Some(w) = s.active_window_mut() {
                                        w.show_help = false;
                                        w.focus.prefix_active = false;
                                    }
                                }
                            }

                            ClientMessage::RenameStart { .. } => {
                                if let Some(ref sess) = attached_session {
                                    let mut s = sess.write().await;
                                    if let Some(w) = s.active_window_mut() {
                                        let focused = w.focus.focused_pane;
                                        let current = w.panes.get(&focused)
                                            .and_then(|p| p.label.clone())
                                            .unwrap_or_default();
                                        w.rename_state = Some(crate::daemon::window::RenameState {
                                            target: crate::daemon::window::RenameTarget::Pane,
                                            buffer: current,
                                        });
                                    }
                                }
                            }

                            ClientMessage::RenameChar(c) => {
                                if let Some(ref sess) = attached_session {
                                    let mut s = sess.write().await;
                                    if let Some(w) = s.active_window_mut()
                                        && let Some(ref mut state) = w.rename_state {
                                            state.buffer.push(c);
                                        }
                                }
                            }

                            ClientMessage::RenameBackspace => {
                                if let Some(ref sess) = attached_session {
                                    let mut s = sess.write().await;
                                    if let Some(w) = s.active_window_mut()
                                        && let Some(ref mut state) = w.rename_state {
                                            state.buffer.pop();
                                        }
                                }
                            }

                            ClientMessage::RenameConfirm { label } => {
                                if let Some(ref sess) = attached_session {
                                    let mut s = sess.write().await;
                                    if let Some(w) = s.active_window_mut() {
                                        if let Some(state) = w.rename_state.take() {
                                            let final_label = if label.is_empty() { state.buffer } else { label };
                                            match state.target {
                                                crate::daemon::window::RenameTarget::Pane => {
                                                    let focused = w.focus.focused_pane;
                                                    if let Some(pane) = w.panes.get_mut(&focused) {
                                                        pane.label = if final_label.is_empty() { None } else { Some(final_label) };
                                                    }
                                                }
                                                crate::daemon::window::RenameTarget::Window => {
                                                    if !final_label.is_empty() {
                                                        w.name = final_label;
                                                    }
                                                }
                                            }
                                        } else {
                                            let focused = w.focus.focused_pane;
                                            if let Some(pane) = w.panes.get_mut(&focused) {
                                                pane.label = if label.is_empty() { None } else { Some(label) };
                                            }
                                        }
                                    }
                                }
                            }

                            ClientMessage::RenameCancel => {
                                if let Some(ref sess) = attached_session {
                                    let mut s = sess.write().await;
                                    if let Some(w) = s.active_window_mut() {
                                        w.rename_state = None;
                                    }
                                }
                            }

                            ClientMessage::WindowRename { label } => {
                                if let Some(ref sess) = attached_session {
                                    let mut s = sess.write().await;
                                    if let Some(w) = s.active_window_mut() {
                                        if label.is_empty() {
                                            let current = w.name.clone();
                                            w.rename_state = Some(crate::daemon::window::RenameState {
                                                target: crate::daemon::window::RenameTarget::Window,
                                                buffer: current,
                                            });
                                        } else {
                                            w.name = label;
                                            w.rename_state = None;
                                        }
                                    }
                                }
                            }

                            ClientMessage::WindowRenameConfirm { label } => {
                                if let Some(ref sess) = attached_session {
                                    let mut s = sess.write().await;
                                    if let Some(w) = s.active_window_mut() {
                                        w.rename_state = None;
                                        if !label.is_empty() {
                                            w.name = label;
                                        }
                                    }
                                }
                            }

                            ClientMessage::WindowNew => {
                                if let Some(ref sess) = attached_session {
                                    let mut s = sess.write().await;
                                    s.new_window();
                                    let _ = sink.send(Frame::from_server(&ServerMessage::Ack)).await;
                                }
                            }

                            ClientMessage::WindowNext => {
                                if let Some(ref sess) = attached_session {
                                    let mut s = sess.write().await;
                                    s.next_window();
                                    let _ = sink.send(Frame::from_server(&ServerMessage::Ack)).await;
                                }
                            }

                            ClientMessage::WindowPrevious => {
                                if let Some(ref sess) = attached_session {
                                    let mut s = sess.write().await;
                                    s.previous_window();
                                    let _ = sink.send(Frame::from_server(&ServerMessage::Ack)).await;
                                }
                            }

                            ClientMessage::WindowSwitch { index } => {
                                if let Some(ref sess) = attached_session {
                                    let mut s = sess.write().await;
                                    s.switch_window(index as usize);
                                    let _ = sink.send(Frame::from_server(&ServerMessage::Ack)).await;
                                }
                            }

                            ClientMessage::WindowClose => {
                                if let Some(ref sess) = attached_session {
                                    let mut s = sess.write().await;
                                    let is_empty = s.close_active_window();
                                    let _ = sink.send(Frame::from_server(&ServerMessage::Ack)).await;
                                    if is_empty {
                                        let sess_name = session_name.clone().unwrap_or_default();
                                        drop(s);
                                        let mut mgr = session_manager.write().await;
                                        let _ = mgr.kill_session(&sess_name).await;
                                        let _ = sink.send(Frame::from_server(&ServerMessage::ExitClient)).await;
                                        break;
                                    }
                                }
                            }

                            ClientMessage::SplitPane { horizontal } => {
                                if let Some(ref sess) = attached_session {
                                    let mut s = sess.write().await;
                                    s.spawn_pane(if horizontal { Split::Horizontal } else { Split::Vertical });
                                    let _ = sink.send(Frame::from_server(&ServerMessage::Ack)).await;
                                }
                            }

                            ClientMessage::KillPane => {
                                if let Some(ref sess) = attached_session {
                                    let mut s = sess.write().await;
                                    s.close_pane();
                                    let _ = sink.send(Frame::from_server(&ServerMessage::Ack)).await;
                                    if s.is_empty() || s.windows.is_empty() {
                                        let sess_name = session_name.clone().unwrap_or_default();
                                        drop(s);
                                        let mut mgr = session_manager.write().await;
                                        let _ = mgr.kill_session(&sess_name).await;
                                        let _ = sink.send(Frame::from_server(&ServerMessage::ExitClient)).await;
                                        break;
                                    }
                                }
                            }

                            ClientMessage::KillServer => {
                                let _ = daemon_event_tx.send(super::DaemonEvent::Shutdown).await;
                            }
                        }

                        if let Some(ref sess) = attached_session {
                            let s = sess.read().await;
                            s.dirty.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    Some(Err(e)) => {
                        eprintln!("client read error: {}", e);
                        break;
                    }
                    None => {
                        // Client disconnected.
                        break;
                    }
                }
            }

            // Periodic render tick — 60 FPS.
            _ = render_tick.tick() => {
                if let (Some(sess), Some(r)) = (&attached_session, &mut renderer) {
                    let current_time = crate::app::statusbar::current_time_hhmm();
                    let s = sess.read().await;
                    if s.windows.is_empty() || s.is_empty() {
                        drop(s);
                        let _ = sink.send(Frame::from_server(&ServerMessage::ExitClient)).await;
                        break;
                    }
                    let is_dirty = s.dirty.load(std::sync::atomic::Ordering::Relaxed);
                    let time_changed = Some(&current_time) != last_rendered_time.as_ref();
                    if is_dirty || time_changed {
                        s.dirty.store(false, std::sync::atomic::Ordering::Relaxed);
                        if let Some(window) = s.active_window() {
                            let frame_data = r.render_frame(&s, window, cols, rows);
                            drop(s);
                            last_rendered_time = Some(current_time);
                            if let Ok(bytes) = frame_data
                                && !bytes.is_empty() {
                                    let msg = ServerMessage::Draw(bytes);
                                    if let Err(e) = sink.send(Frame::from_server(&msg)).await {
                                        eprintln!("fshell-panesd: render send error: {}", e);
                                        break;
                                    }
                                }
                        } else {
                            drop(s);
                        }
                    } else {
                        drop(s);
                    }
                }
            }
        }
    }

    // Cleanup: detach from session on disconnect.
    if let Some(ref sess) = attached_session {
        let mut s = sess.write().await;
        s.attached_client = None;
    }
}

/// Direction for geometric focus navigation.
enum Direction {
    Up,
    Down,
    Left,
    Right,
}

/// Resolve focus to the nearest pane in the given direction using BSP layout geometry.
/// This matches the behavior of the TUI app's `resolve_geometric_focus`.
fn resolve_focus_in_direction(
    session: &mut Session,
    active_window_idx: usize,
    direction: Direction,
) {
    let (cols, rows) = session.terminal_size;
    let pane_rows = rows.saturating_sub(1);
    let area = ratatui::layout::Rect::new(0, 0, cols, pane_rows);
    let window = match session.windows.get(active_window_idx) {
        Some(w) => w,
        None => return,
    };
    let layout = window.bsp.compute_layout(area);
    let focused_pane = window.focus.focused_pane;

    // Find current pane's rect
    let current_rect = match layout.iter().find(|(id, _)| *id == focused_pane) {
        Some((_, rect)) => *rect,
        None => return,
    };

    // Calculate midpoint of current pane
    let mid_x = current_rect.x + current_rect.width / 2;
    let mid_y = current_rect.y + current_rect.height / 2;

    // Find nearest pane in that direction using Euclidean distance
    let mut best: Option<(u32, u32)> = None; // (pane_id, distance_squared)

    for (id, rect) in &layout {
        if *id == focused_pane {
            continue;
        }

        let other_mid_x = rect.x + rect.width / 2;
        let other_mid_y = rect.y + rect.height / 2;

        // Check if other pane is in the correct direction
        let in_direction = match direction {
            Direction::Up => (other_mid_y as i32) < (mid_y as i32),
            Direction::Down => (other_mid_y as i32) > (mid_y as i32),
            Direction::Left => (other_mid_x as i32) < (mid_x as i32),
            Direction::Right => (other_mid_x as i32) > (mid_x as i32),
        };

        if !in_direction {
            continue;
        }

        // Euclidean distance squared (no sqrt needed for comparison)
        let dx = other_mid_x as i32 - mid_x as i32;
        let dy = other_mid_y as i32 - mid_y as i32;
        let dist_sq = (dx * dx + dy * dy) as u32;

        match best {
            None => best = Some((*id, dist_sq)),
            Some((_, best_dist)) if dist_sq < best_dist => best = Some((*id, dist_sq)),
            _ => {}
        }
    }

    if let Some((target_id, _)) = best
        && let Some(window) = session.windows.get_mut(active_window_idx)
    {
        window.focus.set_focused_pane(target_id);
    }
}
