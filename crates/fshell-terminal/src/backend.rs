// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Ratatui backend over fshell's own ANSI terminal primitives.
//!
//! Buffered cell diffs, style changes, clearing, sizing, and the cursor
//! status-report query all go through [`fshell_tty::ansi`] and
//! [`fshell_tty::raw`], so no third-party terminal crate sits between ratatui
//! and the device. Named colors use the compact 16-color codes rather than
//! the 256-color spellings; the rendered result is identical.
//!
//! Unix only.

use std::io::{self, Write};

use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use ratatui::style::{Color, Modifier};

use fshell_tty::{ansi, raw};

/// Ratatui backend writing ANSI sequences to a terminal device.
#[derive(Debug, Default, Clone, Eq, PartialEq, Hash)]
pub struct FshellBackend<W: Write> {
    writer: W,
    cursor_position: Option<Position>,
    output_buffer: Vec<u8>,
}

impl<W: Write> FshellBackend<W> {
    /// Create a backend over `writer`.
    pub const fn new(writer: W) -> Self {
        Self {
            writer,
            cursor_position: None,
            output_buffer: Vec::new(),
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
        let mut fg = Color::Reset;
        let mut bg = Color::Reset;
        let mut underline_color = Color::Reset;
        let mut modifier = Modifier::empty();
        let mut last: Option<Position> = None;
        for (x, y, cell) in content {
            // A cell directly right of the previous one needs no move.
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
        ansi::hide_cursor(&mut self.output_buffer)
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        ansi::show_cursor(&mut self.output_buffer)
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
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
        ansi::move_to(&mut self.output_buffer, x, y)?;
        self.cursor_position = Some(Position { x, y });
        Ok(())
    }

    fn clear(&mut self) -> io::Result<()> {
        self.clear_region(ClearType::All)
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
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
    /// Commit queued terminal operations with one write/flush boundary.
    fn flush_output(&mut self) -> io::Result<()> {
        if !self.output_buffer.is_empty() {
            self.writer.write_all(&self.output_buffer)?;
            self.output_buffer.clear();
        }
        self.writer.flush()
    }
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
