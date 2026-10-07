// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Ratatui backend over fshell's own ANSI terminal primitives.
//!
//! Styles, clearing, and sizing go through [`fshell_tty::ansi`] and
//! [`fshell_tty::raw`], so no third-party terminal crate sits between ratatui
//! and the device. Absolute mode can query the cursor when needed. Inline mode
//! saves an opaque terminal origin, emits each logical row as a stream, and
//! lets the terminal perform soft wrapping; it never queries the cursor.
//! Named colors use compact 16-color codes rather than 256-color spellings.
//!
//! Unix only.

use std::io::{self, Write};

use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::{Cell, CellWidth};
use ratatui::layout::{Position, Size};
use ratatui::style::{Color, Modifier};

use fshell_tty::{ansi, raw};

/// Ratatui backend writing ANSI sequences to a terminal device.
#[derive(Debug, Default, Clone, Eq, PartialEq, Hash)]
pub struct FshellBackend<W: Write> {
    writer: W,
    cursor_position: Option<Position>,
    output_buffer: Vec<u8>,
    cursor_mode: CursorMode,
    cursor_show_pending: bool,
    relative_cells: Vec<Cell>,
}

#[derive(Debug, Default, Clone, Copy, Eq, PartialEq, Hash)]
enum CursorMode {
    #[default]
    Absolute,
    Relative {
        surface_height: u16,
        surface_width: u16,
        /// Physical column of the anchor when known. `None` means a direct-TTY
        /// child may have moved the cursor before fshell reclaimed the TTY.
        origin_column: Option<u16>,
        anchor_saved: bool,
        frame_drawn: bool,
    },
}

/// Unknown or nonzero columns can consume one physical row when logical row
/// zero soft-wraps. Column zero cannot wrap before the renderer emits its
/// explicit CRLF row boundary.
fn wrap_guard_rows(origin_column: Option<u16>) -> u16 {
    u16::from(origin_column != Some(0))
}

/// Number of physical rows after the anchor that belong to the relative
/// surface, including the possible first-row wrap when its column is opaque.
fn physical_rows_below(surface_height: u16, origin_column: Option<u16>) -> u16 {
    surface_height
        .saturating_sub(1)
        .saturating_add(wrap_guard_rows(origin_column))
}

impl<W: Write> FshellBackend<W> {
    /// Create a backend over `writer`.
    pub const fn new(writer: W) -> Self {
        Self {
            writer,
            cursor_position: None,
            output_buffer: Vec::new(),
            cursor_mode: CursorMode::Absolute,
            cursor_show_pending: false,
            relative_cells: Vec::new(),
        }
    }

    /// Create a backend over `writer` with a pre-seeded cursor position.
    ///
    /// Avoids an ANSI DSR (`\x1b[6n`) device query over the wire when the caller
    /// already knows or just positioned the cursor.
    pub const fn with_cursor_position(writer: W, position: Position) -> Self {
        Self {
            writer,
            cursor_position: Some(position),
            output_buffer: Vec::new(),
            cursor_mode: CursorMode::Absolute,
            cursor_show_pending: false,
            relative_cells: Vec::new(),
        }
    }

    /// Create a stream-oriented backend for an inline viewport whose origin
    /// is the terminal's current cursor, including its current column. Call
    /// [`Self::establish_relative_surface`] before constructing a Ratatui
    /// terminal with this backend.
    pub const fn new_cursor_relative(writer: W) -> Self {
        Self::new_cursor_relative_with_origin_column(writer, None)
    }

    /// Create an inline backend with explicit knowledge of the current cursor
    /// column. Use `Some(0)` after an fshell-owned CRLF transition and `None`
    /// after an arbitrary direct-TTY child.
    pub const fn new_cursor_relative_with_origin_column(
        writer: W,
        origin_column: Option<u16>,
    ) -> Self {
        Self {
            writer,
            cursor_position: None,
            output_buffer: Vec::new(),
            cursor_mode: CursorMode::Relative {
                surface_height: 1,
                surface_width: 1,
                origin_column,
                anchor_saved: false,
                frame_drawn: false,
            },
            cursor_show_pending: false,
            relative_cells: Vec::new(),
        }
    }

    /// Resume an inline backend at a saved terminal anchor established by
    /// another backend instance.
    pub fn resume_cursor_relative(writer: W, surface_width: u16, surface_height: u16) -> Self {
        Self::resume_cursor_relative_with_origin_column(writer, surface_width, surface_height, None)
    }

    /// Resume an inline backend while preserving the known-column state of its
    /// saved anchor across a backend replacement.
    pub fn resume_cursor_relative_with_origin_column(
        writer: W,
        surface_width: u16,
        surface_height: u16,
        origin_column: Option<u16>,
    ) -> Self {
        Self {
            writer,
            cursor_position: Some(Position::ORIGIN),
            output_buffer: Vec::new(),
            cursor_mode: CursorMode::Relative {
                surface_height: if surface_height == 0 {
                    1
                } else {
                    surface_height
                },
                surface_width: if surface_width == 0 { 1 } else { surface_width },
                origin_column,
                anchor_saved: true,
                frame_drawn: false,
            },
            cursor_show_pending: false,
            relative_cells: vec![
                Cell::default();
                usize::from(surface_width.max(1))
                    .saturating_mul(usize::from(surface_height.max(1)))
            ],
        }
    }
}

impl<W: Write> Write for FshellBackend<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.output_buffer.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_output()
    }
}

impl<W: Write> Backend for FshellBackend<W> {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let relative = matches!(self.cursor_mode, CursorMode::Relative { .. });
        if relative {
            return self.draw_relative(content);
        }

        let mut fg = Color::Reset;
        let mut bg = Color::Reset;
        let mut underline_color = Color::Reset;
        let mut modifier = Modifier::empty();
        let mut last: Option<Position> = None;
        for (x, y, cell) in content {
            if !matches!(last, Some(previous) if x == previous.x + 1 && y == previous.y) {
                ansi::move_to(&mut self.output_buffer, x, y)?;
            }
            last = Some(Position { x, y });
            if cell.modifier != modifier {
                write_modifier_diff(&mut self.output_buffer, modifier, cell.modifier)?;
                modifier = cell.modifier;
            }
            if cell.fg != fg || cell.bg != bg {
                write_colors(&mut self.output_buffer, cell.fg, cell.bg)?;
                fg = cell.fg;
                bg = cell.bg;
            }
            if cell.underline_color != underline_color {
                write_underline_color(&mut self.output_buffer, cell.underline_color)?;
                underline_color = cell.underline_color;
            }
            self.output_buffer.write_all(cell.symbol().as_bytes())?;
        }
        self.cursor_position = None;
        // Reset colors and attributes as four commands, the shape the
        // reference backend emits at the end of every diff.
        self.output_buffer.write_all(b"\x1b[39m\x1b[49m\x1b[59m")?;
        write_sgr(&mut self.output_buffer, 0)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.cursor_show_pending = false;
        ansi::hide_cursor(&mut self.output_buffer)
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        if matches!(self.cursor_mode, CursorMode::Relative { .. }) {
            self.cursor_show_pending = true;
            return Ok(());
        }
        ansi::show_cursor(&mut self.output_buffer)
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        if let CursorMode::Relative {
            surface_height,
            surface_width,
            ..
        } = self.cursor_mode
        {
            let position = self.cursor_position.ok_or_else(|| {
                io::Error::other("cursor-relative backend has no tracked cursor position")
            });
            return position.map(|position| Position {
                x: position.x.min(surface_width.saturating_sub(1)),
                y: position.y.min(surface_height.saturating_sub(1)),
            });
        }
        if let Some(pos) = self.cursor_position {
            return Ok(pos);
        }
        self.flush_output()?;
        let pos = raw::cursor_position(&mut self.writer).map(|(x, y)| Position { x, y })?;
        self.cursor_position = Some(pos);
        Ok(pos)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let Position { x, y } = position.into();
        if let CursorMode::Relative {
            surface_height,
            surface_width,
            ..
        } = self.cursor_mode
        {
            let position = Position {
                x: x.min(surface_width.saturating_sub(1)),
                y: y.min(surface_height.saturating_sub(1)),
            };
            self.cursor_position = Some(position);
            self.position_relative_cursor(position)?;
        } else {
            ansi::move_to(&mut self.output_buffer, x, y)?;
            self.cursor_position = Some(Position { x, y });
        }
        Ok(())
    }

    fn clear(&mut self) -> io::Result<()> {
        self.clear_region(ClearType::All)
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        if matches!(self.cursor_mode, CursorMode::Relative { .. }) {
            return self.clear_relative_region(clear_type);
        }
        let sequence = match clear_type {
            ClearType::All => ansi::CLEAR_ALL,
            ClearType::AfterCursor => ansi::CLEAR_FROM_CURSOR_DOWN,
            ClearType::BeforeCursor => ansi::CLEAR_FROM_CURSOR_UP,
            ClearType::CurrentLine => ansi::CLEAR_CURRENT_LINE,
            ClearType::UntilNewLine => ansi::CLEAR_UNTIL_NEW_LINE,
        };
        self.output_buffer.write_all(sequence.as_bytes())
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        if matches!(self.cursor_mode, CursorMode::Relative { .. }) {
            return Err(io::Error::other(
                "append_lines is unsupported for a fixed cursor-relative surface",
            ));
        }
        for _ in 0..n {
            self.output_buffer.write_all(b"\n")?;
        }
        if let Some(ref mut pos) = self.cursor_position {
            pos.y = pos.y.saturating_add(n);
        }
        Ok(())
    }

    fn size(&self) -> io::Result<Size> {
        let (width, height) = raw::size()?;
        Ok(Size { width, height })
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        let (columns, rows, width, height) = raw::window_size()?;
        Ok(WindowSize {
            columns_rows: Size {
                width: columns,
                height: rows,
            },
            pixels: Size { width, height },
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_output()
    }
}

impl<W: Write> FshellBackend<W> {
    fn draw_relative<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let CursorMode::Relative {
            surface_height,
            surface_width,
            ..
        } = self.cursor_mode
        else {
            return Err(io::Error::other("inline draw requires a relative backend"));
        };

        let expected_cells = usize::from(surface_width).saturating_mul(usize::from(surface_height));
        if self.relative_cells.len() != expected_cells {
            self.relative_cells.resize(expected_cells, Cell::default());
        }
        for (x, y, cell) in content {
            if x < surface_width && y < surface_height {
                let index = usize::from(y) * usize::from(surface_width) + usize::from(x);
                self.relative_cells[index] = cell.clone();
            }
        }

        self.cursor_show_pending = false;
        ansi::hide_cursor(&mut self.output_buffer)?;
        self.restore_relative_anchor()?;
        let frame_drawn = matches!(
            self.cursor_mode,
            CursorMode::Relative {
                frame_drawn: true,
                ..
            }
        );
        if frame_drawn {
            let origin_column = match self.cursor_mode {
                CursorMode::Relative { origin_column, .. } => origin_column,
                CursorMode::Absolute => None,
            };
            self.clear_relative_rows_below(physical_rows_below(surface_height, origin_column))?;
            self.restore_relative_anchor()?;
        }
        self.output_buffer.write_all(b"\x1b[0m")?;

        let mut sgr = SgrState::default();
        for row in 0..surface_height {
            let row_extent = relative_row_extent(&self.relative_cells, surface_width, row);
            append_relative_cells(
                &mut self.output_buffer,
                &self.relative_cells,
                surface_width,
                row,
                row_extent,
                &mut sgr,
            )?;
            if row + 1 < surface_height {
                // CR cancels a pending wrap at the right margin; LF advances
                // exactly one physical row. The next logical row begins at 0.
                self.output_buffer.write_all(b"\r\n")?;
            }
        }
        reset_sgr(&mut self.output_buffer, &mut sgr)?;
        self.cursor_position = Some(Position::ORIGIN);
        if let CursorMode::Relative { frame_drawn, .. } = &mut self.cursor_mode {
            *frame_drawn = true;
        }
        Ok(())
    }

    fn position_relative_cursor(&mut self, target: Position) -> io::Result<()> {
        let CursorMode::Relative {
            surface_height,
            surface_width,
            ..
        } = self.cursor_mode
        else {
            return Err(io::Error::other(
                "inline cursor requires a relative backend",
            ));
        };
        let target = Position {
            x: target.x.min(surface_width.saturating_sub(1)),
            y: target.y.min(surface_height.saturating_sub(1)),
        };

        self.restore_relative_anchor()?;
        self.output_buffer.write_all(b"\x1b[0m")?;
        let mut sgr = SgrState::default();
        for row in 0..=target.y {
            let end = if row == target.y {
                target.x
            } else {
                relative_row_extent(&self.relative_cells, surface_width, row)
            };
            append_relative_cells(
                &mut self.output_buffer,
                &self.relative_cells,
                surface_width,
                row,
                end,
                &mut sgr,
            )?;
            if row < target.y {
                self.output_buffer.write_all(b"\r\n")?;
            }
        }
        reset_sgr(&mut self.output_buffer, &mut sgr)?;
        self.cursor_position = Some(target);
        Ok(())
    }

    /// Reserve a cursor-relative surface below the live cursor and save its
    /// origin without assuming that it starts in column zero. Rows are emitted
    /// as a stream from this origin so the terminal itself decides soft wraps.
    pub fn establish_relative_surface(
        &mut self,
        surface_width: u16,
        height: u16,
    ) -> io::Result<()> {
        let CursorMode::Relative { anchor_saved, .. } = self.cursor_mode else {
            return Err(io::Error::other(
                "cannot establish a relative surface on an absolute backend",
            ));
        };
        if anchor_saved {
            return Err(io::Error::other(
                "cursor-relative surface already has an anchor",
            ));
        }

        let (_, terminal_height) = raw::size()?;
        let origin_column = match self.cursor_mode {
            CursorMode::Relative { origin_column, .. } => origin_column,
            CursorMode::Absolute => None,
        };
        let wrap_guard = wrap_guard_rows(origin_column);
        let max_surface_height = terminal_height.saturating_sub(wrap_guard).max(1);
        let height = height.max(1).min(max_surface_height);
        ansi::hide_cursor(&mut self.output_buffer)?;
        self.cursor_mode = CursorMode::Relative {
            surface_height: height,
            surface_width: surface_width.max(1),
            origin_column,
            anchor_saved: false,
            frame_drawn: false,
        };
        self.cursor_position = Some(Position::ORIGIN);
        self.relative_cells = vec![
            Cell::default();
            usize::from(surface_width.max(1))
                .saturating_mul(usize::from(height))
        ];
        // A child-owned cursor can make logical row zero wrap once. Keep an
        // extra physical row only while the origin column is unknown or
        // nonzero; fshell-owned column-zero transitions use every row.
        self.reserve_relative_rows(height.saturating_add(wrap_guard))?;
        self.save_relative_anchor()?;
        self.clear_relative_rows_below(physical_rows_below(height, origin_column))?;
        self.restore_relative_anchor()?;
        self.flush_output()
    }

    /// Clear the old owned rows and establish a new local origin after a
    /// viewport resize. The first row may occupy one extra physical row because
    /// its starting column is opaque.
    pub fn reanchor_relative_surface(
        &mut self,
        surface_width: u16,
        surface_height: u16,
    ) -> io::Result<()> {
        let CursorMode::Relative {
            surface_height: old_height,
            surface_width: old_width,
            origin_column,
            ..
        } = self.cursor_mode
        else {
            return Err(io::Error::other(
                "cannot reanchor an absolute terminal backend",
            ));
        };
        let (_, terminal_height) = raw::size()?;
        let wrap_guard = wrap_guard_rows(origin_column);
        let max_surface_height = terminal_height.saturating_sub(wrap_guard).max(1);
        let surface_height = surface_height.max(1).min(max_surface_height);
        ansi::hide_cursor(&mut self.output_buffer)?;
        self.restore_relative_anchor()?;
        self.clear_relative_rows_below(self.rows_below_after_resize(
            old_width,
            old_height,
            surface_width.max(1),
            origin_column,
        ))?;
        self.restore_relative_anchor()?;
        self.cursor_mode = CursorMode::Relative {
            surface_height,
            surface_width: surface_width.max(1),
            origin_column,
            anchor_saved: false,
            frame_drawn: false,
        };
        self.cursor_position = Some(Position::ORIGIN);
        self.relative_cells = vec![
            Cell::default();
            usize::from(surface_width.max(1))
                .saturating_mul(usize::from(surface_height))
        ];
        self.reserve_relative_rows(surface_height.saturating_add(wrap_guard))?;
        self.save_relative_anchor()?;
        self.clear_relative_rows_below(physical_rows_below(surface_height, origin_column))?;
        self.restore_relative_anchor()?;
        self.flush_output()
    }

    /// Commit transcript lines at the saved prompt origin. The next editor
    /// anchors at the current terminal cursor, including any column left by a
    /// child command.
    pub fn commit_relative_lines(&mut self, lines: &[String]) -> io::Result<()> {
        ansi::hide_cursor(&mut self.output_buffer)?;
        self.restore_relative_anchor()?;
        let surface_height = self.relative_surface_height()?;
        if matches!(
            self.cursor_mode,
            CursorMode::Relative {
                frame_drawn: true,
                ..
            }
        ) {
            let origin_column = match self.cursor_mode {
                CursorMode::Relative { origin_column, .. } => origin_column,
                CursorMode::Absolute => None,
            };
            self.clear_relative_rows_below(physical_rows_below(surface_height, origin_column))?;
        }
        self.restore_relative_anchor()?;
        for line in lines {
            let terminal_line = line.replace("\r\n", "\n").replace('\n', "\r\n");
            self.output_buffer.write_all(terminal_line.as_bytes())?;
            self.output_buffer.write_all(b"\x1b[0m\r\n")?;
        }
        self.flush_output()?;
        if let CursorMode::Relative { anchor_saved, .. } = &mut self.cursor_mode {
            *anchor_saved = false;
        }
        if let CursorMode::Relative { frame_drawn, .. } = &mut self.cursor_mode {
            *frame_drawn = false;
        }
        if !lines.is_empty()
            && let CursorMode::Relative { origin_column, .. } = &mut self.cursor_mode
        {
            *origin_column = Some(0);
        }
        self.cursor_position = None;
        self.relative_cells.clear();
        Ok(())
    }

    /// Remove the live editor surface without disturbing terminal history
    /// above its origin.
    pub fn clear_relative_surface(&mut self) -> io::Result<()> {
        ansi::hide_cursor(&mut self.output_buffer)?;
        self.restore_relative_anchor()?;
        let surface_height = self.relative_surface_height()?;
        if matches!(
            self.cursor_mode,
            CursorMode::Relative {
                frame_drawn: true,
                ..
            }
        ) {
            let origin_column = match self.cursor_mode {
                CursorMode::Relative { origin_column, .. } => origin_column,
                CursorMode::Absolute => None,
            };
            self.clear_relative_rows_below(physical_rows_below(surface_height, origin_column))?;
        }
        self.restore_relative_anchor()?;
        self.flush_output()?;
        if let CursorMode::Relative { anchor_saved, .. } = &mut self.cursor_mode {
            *anchor_saved = false;
        }
        if let CursorMode::Relative { frame_drawn, .. } = &mut self.cursor_mode {
            *frame_drawn = false;
        }
        self.cursor_position = None;
        self.relative_cells.clear();
        Ok(())
    }

    fn reserve_relative_rows(&mut self, height: u16) -> io::Result<()> {
        // Relative cursor motion clears a terminal's deferred-wrap state. The
        // saved origin is therefore a local position, not a complete snapshot
        // of all emulator state left by an arbitrary child process.
        for _ in 1..height {
            // IND advances vertically without applying a terminal's newline
            // mode, which can turn LF into CRLF and lose the opaque column.
            self.output_buffer.write_all(b"\x1bD")?;
        }
        if height > 1 {
            write!(self.output_buffer, "\x1b[{}A", height - 1)?;
        }
        self.cursor_position = Some(Position::ORIGIN);
        Ok(())
    }

    fn relative_surface_height(&self) -> io::Result<u16> {
        match self.cursor_mode {
            CursorMode::Relative { surface_height, .. } => Ok(surface_height),
            CursorMode::Absolute => Err(io::Error::other(
                "cursor-relative operation requires a cursor-relative backend",
            )),
        }
    }

    /// Clear only the editor's stream footprint. The first row is erased from
    /// its saved starting column; rows below it are full-width owned rows.
    fn clear_relative_rows_below(&mut self, rows_below: u16) -> io::Result<()> {
        self.output_buffer.write_all(b"\x1b[0m")?;
        self.output_buffer
            .write_all(ansi::CLEAR_UNTIL_NEW_LINE.as_bytes())?;
        for _ in 0..rows_below {
            self.output_buffer.write_all(b"\r\x1b[1B\x1b[2K")?;
        }
        Ok(())
    }

    fn rows_below_after_resize(
        &self,
        old_width: u16,
        old_height: u16,
        new_width: u16,
        origin_column: Option<u16>,
    ) -> u16 {
        let old_width = u32::from(old_width.max(1));
        let new_width = u32::from(new_width.max(1));
        let logical_rows = u32::from(old_height.max(1));
        let rows_per_logical = old_width.div_ceil(new_width);
        let start_column = origin_column
            .map(u32::from)
            .unwrap_or_else(|| new_width.saturating_sub(1))
            .min(new_width.saturating_sub(1));
        let first_row = old_width.saturating_add(start_column).div_ceil(new_width);
        let physical_rows = first_row.saturating_add(
            logical_rows
                .saturating_sub(1)
                .saturating_mul(rows_per_logical),
        );
        physical_rows.saturating_sub(1).min(u32::from(u16::MAX)) as u16
    }

    fn save_relative_anchor(&mut self) -> io::Result<()> {
        self.output_buffer.write_all(b"\x1b7")?;
        if let CursorMode::Relative { anchor_saved, .. } = &mut self.cursor_mode {
            *anchor_saved = true;
        }
        self.cursor_position = Some(Position::ORIGIN);
        Ok(())
    }

    fn restore_relative_anchor(&mut self) -> io::Result<()> {
        let CursorMode::Relative { anchor_saved, .. } = self.cursor_mode else {
            return Err(io::Error::other(
                "cannot restore a relative anchor on an absolute backend",
            ));
        };
        if !anchor_saved {
            return Err(io::Error::other(
                "cursor-relative surface has no saved origin",
            ));
        }
        self.output_buffer.write_all(b"\x1b8")?;
        self.cursor_position = Some(Position::ORIGIN);
        Ok(())
    }

    fn clear_relative_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        let cursor = self.cursor_position.unwrap_or(Position::ORIGIN);
        let (width, height) = match self.cursor_mode {
            CursorMode::Relative {
                surface_height,
                surface_width,
                ..
            } => (surface_width as usize, surface_height as usize),
            CursorMode::Absolute => return Ok(()),
        };
        match clear_type {
            ClearType::All => self.relative_cells.fill(Cell::default()),
            ClearType::AfterCursor => {
                for y in cursor.y as usize..height {
                    let start = if y == cursor.y as usize {
                        cursor.x as usize
                    } else {
                        0
                    };
                    for x in start..width {
                        if let Some(cell) = self.relative_cells.get_mut(y * width + x) {
                            *cell = Cell::default();
                        }
                    }
                }
            }
            ClearType::BeforeCursor => {
                for y in 0..=cursor.y as usize {
                    let end = if y == cursor.y as usize {
                        cursor.x as usize + 1
                    } else {
                        width
                    };
                    for x in 0..end.min(width) {
                        if let Some(cell) = self.relative_cells.get_mut(y * width + x) {
                            *cell = Cell::default();
                        }
                    }
                }
            }
            ClearType::CurrentLine => {
                let y = cursor.y as usize;
                for x in 0..width {
                    if let Some(cell) = self.relative_cells.get_mut(y * width + x) {
                        *cell = Cell::default();
                    }
                }
            }
            ClearType::UntilNewLine => {
                let y = cursor.y as usize;
                for x in cursor.x as usize..width {
                    if let Some(cell) = self.relative_cells.get_mut(y * width + x) {
                        *cell = Cell::default();
                    }
                }
            }
        }
        Ok(())
    }

    /// Commit queued terminal operations with one write/flush boundary.
    fn flush_output(&mut self) -> io::Result<()> {
        if self.cursor_show_pending {
            ansi::show_cursor(&mut self.output_buffer)?;
            self.cursor_show_pending = false;
        }
        if !self.output_buffer.is_empty() {
            self.writer.write_all(&self.output_buffer)?;
            self.output_buffer.clear();
        }
        self.writer.flush()
    }
}

#[derive(Clone, Copy)]
struct SgrState {
    fg: Color,
    bg: Color,
    underline_color: Color,
    modifier: Modifier,
}

impl Default for SgrState {
    fn default() -> Self {
        Self {
            fg: Color::Reset,
            bg: Color::Reset,
            underline_color: Color::Reset,
            modifier: Modifier::empty(),
        }
    }
}

fn append_relative_cells(
    out: &mut Vec<u8>,
    cells: &[Cell],
    width: u16,
    row: u16,
    end_x: u16,
    sgr: &mut SgrState,
) -> io::Result<()> {
    let row_start = usize::from(row) * usize::from(width);
    let end_x = end_x.min(width);
    let mut x = 0u16;
    while x < end_x {
        let Some(cell) = cells.get(row_start + usize::from(x)) else {
            break;
        };
        let cell_width = cell.cell_width();
        if cell_width == 0 {
            // Ratatui represents the second cell of a wide grapheme as a
            // zero-width continuation. Empty continuations must not emit a
            // second terminal glyph; standalone combining text is retained.
            if !cell.symbol().is_empty() {
                append_cell_style(out, cell, sgr)?;
                out.write_all(cell.symbol().as_bytes())?;
            }
            x = x.saturating_add(1);
            continue;
        }
        if x.saturating_add(cell_width) > end_x {
            break;
        }
        append_cell_style(out, cell, sgr)?;
        let symbol = cell.symbol();
        if symbol.is_empty() {
            out.write_all(b" ")?;
        } else {
            out.write_all(symbol.as_bytes())?;
        }
        x = x.saturating_add(cell_width);
    }

    Ok(())
}

fn relative_row_extent(cells: &[Cell], width: u16, row: u16) -> u16 {
    let row_start = usize::from(row) * usize::from(width);
    let mut extent = 0;
    for x in 0..width {
        let Some(cell) = cells.get(row_start + usize::from(x)) else {
            break;
        };
        let is_default_blank = cell.symbol() == " "
            && cell.fg == Color::Reset
            && cell.bg == Color::Reset
            && cell.underline_color == Color::Reset
            && cell.modifier.is_empty();
        if !is_default_blank {
            extent = extent.max(x.saturating_add(cell.cell_width().max(1)));
        }
    }
    extent.min(width)
}

fn append_cell_style(out: &mut Vec<u8>, cell: &Cell, sgr: &mut SgrState) -> io::Result<()> {
    if cell.modifier != sgr.modifier {
        write_modifier_diff(out, sgr.modifier, cell.modifier)?;
        sgr.modifier = cell.modifier;
    }
    if cell.fg != sgr.fg || cell.bg != sgr.bg {
        write_colors(out, cell.fg, cell.bg)?;
        sgr.fg = cell.fg;
        sgr.bg = cell.bg;
    }
    if cell.underline_color != sgr.underline_color {
        write_underline_color(out, cell.underline_color)?;
        sgr.underline_color = cell.underline_color;
    }
    Ok(())
}

fn reset_sgr(out: &mut Vec<u8>, sgr: &mut SgrState) -> io::Result<()> {
    out.write_all(b"\x1b[39m\x1b[49m\x1b[59m")?;
    write_sgr(out, 0)?;
    *sgr = SgrState::default();
    Ok(())
}

/// Write one SGR parameter sequence.
fn write_sgr(out: &mut impl Write, code: u16) -> io::Result<()> {
    write!(out, "\x1b[{code}m")
}

/// Write the foreground and background colors as one combined sequence.
fn write_colors(out: &mut impl Write, fg: Color, bg: Color) -> io::Result<()> {
    out.write_all(b"\x1b[")?;
    write_color(out, fg, Layer::Foreground)?;
    out.write_all(b";")?;
    write_color(out, bg, Layer::Background)?;
    out.write_all(b"m")
}

/// Which layer a color parameter addresses.
enum Layer {
    Foreground,
    Background,
}

/// Write a color's SGR parameters: named colors select the dark or bright
/// half of the 8-color palette, `Indexed` uses the 256-color form, and `Rgb`
/// the true-color form.
fn write_color(out: &mut impl Write, color: Color, layer: Layer) -> io::Result<()> {
    let (base, reset, extended) = match layer {
        Layer::Foreground => (30, 39, 38),
        Layer::Background => (40, 49, 48),
    };
    match color {
        Color::Reset => write!(out, "{reset}"),
        Color::Black => write!(out, "{}", base),
        Color::Red => write!(out, "{}", base + 1),
        Color::Green => write!(out, "{}", base + 2),
        Color::Yellow => write!(out, "{}", base + 3),
        Color::Blue => write!(out, "{}", base + 4),
        Color::Magenta => write!(out, "{}", base + 5),
        Color::Cyan => write!(out, "{}", base + 6),
        Color::Gray => write!(out, "{}", base + 7),
        Color::DarkGray => write!(out, "{}", base + 60),
        Color::LightRed => write!(out, "{}", base + 61),
        Color::LightGreen => write!(out, "{}", base + 62),
        Color::LightYellow => write!(out, "{}", base + 63),
        Color::LightBlue => write!(out, "{}", base + 64),
        Color::LightMagenta => write!(out, "{}", base + 65),
        Color::LightCyan => write!(out, "{}", base + 66),
        Color::White => write!(out, "{}", base + 67),
        Color::Indexed(index) => write!(out, "{extended};5;{index}"),
        Color::Rgb(r, g, b) => write!(out, "{extended};2;{r};{g};{b}"),
    }
}

/// Write `SetUnderlineColor(color)`: the `58` form for colors, `59` for a
/// reset. Named colors are expressed by palette index.
fn write_underline_color(out: &mut impl Write, color: Color) -> io::Result<()> {
    match color {
        Color::Reset => write_sgr(out, 59),
        Color::Indexed(index) => write!(out, "\x1b[58;5;{index}m"),
        Color::Rgb(r, g, b) => write!(out, "\x1b[58;2;{r};{g};{b}m"),
        named => write!(out, "\x1b[58;5;{}m", palette_index(named)),
    }
}

/// The 256-color palette index of a named color, in standard palette order.
fn palette_index(color: Color) -> u16 {
    match color {
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::Gray => 7,
        Color::DarkGray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        Color::White => 15,
        Color::Reset | Color::Indexed(_) | Color::Rgb(_, _, _) => 0,
    }
}

/// Write the attribute transitions between two modifier sets: removals
/// first, then the shared intensity reset with its re-applications, then the
/// additions.
fn write_modifier_diff(out: &mut impl Write, from: Modifier, to: Modifier) -> io::Result<()> {
    let removed = from & !to;
    if removed.contains(Modifier::REVERSED) {
        write_sgr(out, 27)?;
    }

    // Bold and dim share one intensity reset; anything still wanted is
    // re-applied immediately after it.
    let reset_intensity = removed.intersects(Modifier::BOLD | Modifier::DIM);
    if reset_intensity {
        write_sgr(out, 22)?;
        if to.contains(Modifier::DIM) {
            write_sgr(out, 2)?;
        }
        if to.contains(Modifier::BOLD) {
            write_sgr(out, 1)?;
        }
    }

    if removed.contains(Modifier::ITALIC) {
        write_sgr(out, 23)?;
    }
    if removed.contains(Modifier::UNDERLINED) {
        write_sgr(out, 24)?;
    }
    if removed.contains(Modifier::CROSSED_OUT) {
        write_sgr(out, 29)?;
    }
    if removed.contains(Modifier::HIDDEN) {
        write_sgr(out, 28)?;
    }
    if removed.intersects(Modifier::SLOW_BLINK | Modifier::RAPID_BLINK) {
        write_sgr(out, 25)?;
    }

    let added = to & !from;
    if added.contains(Modifier::REVERSED) {
        write_sgr(out, 7)?;
    }
    if added.contains(Modifier::BOLD) && !reset_intensity {
        write_sgr(out, 1)?;
    }
    if added.contains(Modifier::ITALIC) {
        write_sgr(out, 3)?;
    }
    if added.contains(Modifier::UNDERLINED) {
        write_sgr(out, 4)?;
    }
    if added.contains(Modifier::DIM) && !reset_intensity {
        write_sgr(out, 2)?;
    }
    if added.contains(Modifier::CROSSED_OUT) {
        write_sgr(out, 9)?;
    }
    if added.contains(Modifier::HIDDEN) {
        write_sgr(out, 8)?;
    }
    if added.contains(Modifier::SLOW_BLINK) {
        write_sgr(out, 5)?;
    }
    if added.contains(Modifier::RAPID_BLINK) {
        write_sgr(out, 6)?;
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Style;

    /// Draw `buffer` through the backend, returning the emitted bytes.
    fn draw(buffer: &Buffer) -> Vec<u8> {
        let width = buffer.area.width as usize;
        let mut backend = FshellBackend::new(Vec::new());
        Backend::draw(
            &mut backend,
            buffer
                .content
                .iter()
                .enumerate()
                .map(|(index, cell)| ((index % width) as u16, (index / width) as u16, cell)),
        )
        .unwrap();
        Backend::flush(&mut backend).unwrap();
        backend.writer
    }

    /// Draw one cell and return the emitted bytes.
    fn draw_cell(cell: ratatui::buffer::Cell) -> String {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 1, 1));
        buffer.content[0] = cell;
        String::from_utf8(draw(&buffer)).unwrap()
    }

    #[test]
    fn plain_cell_moves_and_resets() {
        // A cell with no symbol draws a space, as ratatui defines it.
        assert_eq!(
            draw_cell(ratatui::buffer::Cell::default()),
            "\x1b[1;1H \x1b[39m\x1b[49m\x1b[59m\x1b[0m"
        );
        // A reset-colored cell is indistinguishable from the default state.
        let mut cell = ratatui::buffer::Cell::default();
        cell.set_symbol("x");
        assert_eq!(draw_cell(cell), "\x1b[1;1Hx\x1b[39m\x1b[49m\x1b[59m\x1b[0m");
    }

    #[test]
    fn contiguous_cells_skip_the_move() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 3, 1));
        buffer.set_string(0, 0, "abc", Style::default());
        let output = String::from_utf8(draw(&buffer)).unwrap();
        assert_eq!(output, "\x1b[1;1Habc\x1b[39m\x1b[49m\x1b[59m\x1b[0m");
    }

    #[test]
    fn styled_cell_emits_attribute_then_colors() {
        let mut cell = ratatui::buffer::Cell::default();
        cell.set_symbol("x");
        cell.fg = Color::Red;
        cell.bg = Color::Blue;
        cell.modifier = Modifier::BOLD;
        assert_eq!(
            draw_cell(cell),
            "\x1b[1;1H\x1b[1m\x1b[31;44mx\x1b[39m\x1b[49m\x1b[59m\x1b[0m"
        );
    }

    #[test]
    fn color_parameters_use_standard_palette_codes() {
        let cases = [
            (Color::Black, "30", "40"),
            (Color::Red, "31", "41"),
            (Color::Green, "32", "42"),
            (Color::Yellow, "33", "43"),
            (Color::Blue, "34", "44"),
            (Color::Magenta, "35", "45"),
            (Color::Cyan, "36", "46"),
            (Color::Gray, "37", "47"),
            (Color::DarkGray, "90", "100"),
            (Color::LightRed, "91", "101"),
            (Color::LightGreen, "92", "102"),
            (Color::LightYellow, "93", "103"),
            (Color::LightBlue, "94", "104"),
            (Color::LightMagenta, "95", "105"),
            (Color::LightCyan, "96", "106"),
            (Color::White, "97", "107"),
            (Color::Indexed(200), "38;5;200", "48;5;200"),
            (Color::Rgb(1, 2, 3), "38;2;1;2;3", "48;2;1;2;3"),
        ];
        for (color, foreground, background) in cases {
            let mut cell = ratatui::buffer::Cell::default();
            cell.set_symbol("x");
            cell.fg = color;
            cell.bg = color;
            assert_eq!(
                draw_cell(cell),
                format!("\x1b[1;1H\x1b[{foreground};{background}mx\x1b[39m\x1b[49m\x1b[59m\x1b[0m"),
                "color {color:?}"
            );
        }
    }

    #[test]
    fn underline_colors_use_the_58_form() {
        let mut cell = ratatui::buffer::Cell::default();
        cell.set_symbol("x");
        cell.underline_color = Color::Indexed(4);
        assert_eq!(
            draw_cell(cell),
            "\x1b[1;1H\x1b[58;5;4mx\x1b[39m\x1b[49m\x1b[59m\x1b[0m"
        );
    }

    #[test]
    fn named_underline_colors_use_palette_indexes() {
        let cases = [
            (Color::Reset, "\x1b[59m"),
            (Color::Black, "\x1b[58;5;0m"),
            (Color::LightRed, "\x1b[58;5;9m"),
            (Color::White, "\x1b[58;5;15m"),
            (Color::Indexed(200), "\x1b[58;5;200m"),
            (Color::Rgb(1, 2, 3), "\x1b[58;2;1;2;3m"),
        ];
        for (color, expected) in cases {
            let mut out = Vec::new();
            write_underline_color(&mut out, color).unwrap();
            assert_eq!(String::from_utf8(out).unwrap(), expected, "color {color:?}");
        }
    }

    #[test]
    fn modifier_diff_orders_resets_before_additions() {
        // Intensity reset re-applies the surviving half.
        assert_eq!(diff(Modifier::BOLD, Modifier::DIM), "\x1b[22m\x1b[2m");
        assert_eq!(
            diff(
                Modifier::HIDDEN | Modifier::DIM,
                Modifier::BOLD | Modifier::DIM
            ),
            "\x1b[28m\x1b[1m"
        );
        // Removals precede additions.
        assert_eq!(
            diff(Modifier::SLOW_BLINK, Modifier::RAPID_BLINK),
            "\x1b[25m\x1b[6m"
        );
        assert_eq!(diff(Modifier::empty(), Modifier::REVERSED), "\x1b[7m");
        assert_eq!(diff(Modifier::REVERSED, Modifier::empty()), "\x1b[27m");
        assert_eq!(diff(Modifier::UNDERLINED, Modifier::empty()), "\x1b[24m");
        assert_eq!(diff(Modifier::empty(), Modifier::CROSSED_OUT), "\x1b[9m");
        // Equal sets emit nothing.
        assert_eq!(diff(Modifier::ITALIC, Modifier::ITALIC), "");
    }

    fn diff(from: Modifier, to: Modifier) -> String {
        let mut out = Vec::new();
        write_modifier_diff(&mut out, from, to).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn clear_types_map_to_ansi() {
        let cases = [
            (ClearType::All, "\x1b[2J"),
            (ClearType::AfterCursor, "\x1b[J"),
            (ClearType::BeforeCursor, "\x1b[1J"),
            (ClearType::CurrentLine, "\x1b[2K"),
            (ClearType::UntilNewLine, "\x1b[K"),
        ];
        for (clear_type, expected) in cases {
            let mut backend = FshellBackend::new(Vec::new());
            backend.clear_region(clear_type).unwrap();
            Backend::flush(&mut backend).unwrap();
            assert_eq!(String::from_utf8(backend.writer).unwrap(), expected);
        }
    }

    #[test]
    fn append_lines_writes_newlines() {
        let mut backend = FshellBackend::new(Vec::new());
        backend.append_lines(2).unwrap();
        Backend::flush(&mut backend).unwrap();
        assert_eq!(backend.writer, b"\n\n");
    }

    #[test]
    fn test_with_cursor_position_avoids_query() {
        let mut backend = FshellBackend::with_cursor_position(Vec::new(), Position { x: 5, y: 12 });
        let pos = backend
            .get_cursor_position()
            .expect("should return cached position");
        assert_eq!(pos, Position { x: 5, y: 12 });
        // The writer had no DSR query emitted because the position was known
        assert!(backend.writer.is_empty());

        backend
            .append_lines(3)
            .expect("append_lines should succeed");
        let pos2 = backend
            .get_cursor_position()
            .expect("should return updated cached position");
        assert_eq!(pos2, Position { x: 5, y: 15 });

        backend
            .set_cursor_position(Position { x: 1, y: 2 })
            .expect("set_cursor_position should succeed");
        let pos3 = backend
            .get_cursor_position()
            .expect("should return set position");
        assert_eq!(pos3, Position { x: 1, y: 2 });
        Backend::flush(&mut backend).expect("queued terminal operations should flush");
        assert_eq!(backend.writer, b"\n\n\n\x1b[3;2H");
    }

    #[derive(Default)]
    struct WriteCounts {
        writes: usize,
        flushes: usize,
        bytes: Vec<u8>,
    }

    struct CountingWriter(std::sync::Arc<std::sync::Mutex<WriteCounts>>);

    impl Write for CountingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let mut counts = self.0.lock().expect("count lock should not be poisoned");
            counts.writes += 1;
            counts.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.0
                .lock()
                .expect("count lock should not be poisoned")
                .flushes += 1;
            Ok(())
        }
    }

    #[test]
    fn frame_operations_are_committed_together() {
        let counts = std::sync::Arc::new(std::sync::Mutex::new(WriteCounts::default()));
        let mut backend = FshellBackend::with_cursor_position(
            CountingWriter(counts.clone()),
            Position { x: 0, y: 0 },
        );
        let mut cell = ratatui::buffer::Cell::default();
        cell.set_symbol("x");

        Backend::draw(&mut backend, std::iter::once((0, 0, &cell)))
            .expect("frame cells should be buffered");
        Backend::show_cursor(&mut backend).expect("cursor visibility should be buffered");
        Backend::set_cursor_position(&mut backend, Position { x: 1, y: 0 })
            .expect("cursor movement should be buffered");

        {
            let counts = counts.lock().expect("count lock should not be poisoned");
            assert_eq!(counts.writes, 0);
            assert_eq!(counts.flushes, 0);
        }

        Backend::flush(&mut backend).expect("frame should be committed");
        let counts = counts.lock().expect("count lock should not be poisoned");
        assert_eq!(counts.writes, 1);
        assert_eq!(counts.flushes, 1);
        let output = String::from_utf8_lossy(&counts.bytes);
        let drawn_cell = output.find('x').expect("drawn cell should be present");
        let shown_cursor = output
            .find("\x1b[?25h")
            .expect("cursor show sequence should be present");
        let moved_cursor = output
            .find("\x1b[1;2H")
            .expect("cursor move should be present");
        assert!(drawn_cell < shown_cursor && shown_cursor < moved_cursor);
    }
}
