// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Ratatui backend over fshell's own ANSI terminal primitives.
//!
//! Byte-for-byte replacement for `ratatui-crossterm`'s backend: buffered
//! cell diffs, style changes, clearing, sizing, and the cursor status-report
//! query all go through [`crate::ansi`] and [`crate::raw`], so no third-party
//! terminal crate sits between ratatui and the device.
//!
//! Unix only.

use std::io::{self, Write};

use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use ratatui::style::{Color, Modifier};

use crate::ansi;
use crate::raw;

/// Ratatui backend writing ANSI sequences to a terminal device.
#[derive(Debug, Default, Clone, Eq, PartialEq, Hash)]
pub struct FshellBackend<W: Write> {
    writer: W,
}

impl<W: Write> FshellBackend<W> {
    /// Create a backend over `writer`.
    pub const fn new(writer: W) -> Self {
        Self { writer }
    }
}

impl<W: Write> Write for FshellBackend<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writer.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
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
        let mut modifier = Modifier::empty();
        let mut last: Option<Position> = None;
        for (x, y, cell) in content {
            // A cell directly right of the previous one needs no move.
            if !matches!(last, Some(previous) if x == previous.x + 1 && y == previous.y) {
                ansi::move_to(&mut self.writer, x, y)?;
            }
            last = Some(Position { x, y });
            if cell.modifier != modifier {
                write_modifier_diff(&mut self.writer, modifier, cell.modifier)?;
                modifier = cell.modifier;
            }
            if cell.fg != fg || cell.bg != bg {
                write_colors(&mut self.writer, cell.fg, cell.bg)?;
                fg = cell.fg;
                bg = cell.bg;
            }
            self.writer.write_all(cell.symbol().as_bytes())?;
        }
        // Reset colors and attributes as three commands, the shape
        // `ratatui-crossterm` emits at the end of every diff.
        self.writer.write_all(b"\x1b[39m\x1b[49m")?;
        write_sgr(&mut self.writer, 0)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        ansi::hide_cursor(&mut self.writer)
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        ansi::show_cursor(&mut self.writer)
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        raw::cursor_position(&mut self.writer).map(|(x, y)| Position { x, y })
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let Position { x, y } = position.into();
        ansi::move_to(&mut self.writer, x, y)?;
        self.writer.flush()
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
        self.writer.write_all(sequence.as_bytes())?;
        self.writer.flush()
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        for _ in 0..n {
            self.writer.write_all(b"\n")?;
        }
        self.writer.flush()
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
        self.writer.flush()
    }
}

/// Write one SGR parameter sequence.
fn write_sgr(out: &mut impl Write, code: u16) -> io::Result<()> {
    write!(out, "\x1b[{code}m")
}

/// Write `SetColors(fg, bg)` as the single combined sequence crossterm emits.
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

/// Write a color's SGR parameters, matching crossterm's color table: the
/// named colors select the dark or bright half of the 8-color palette,
/// `Indexed` uses the 256-color form, and `Rgb` the true-color form.
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

/// Write the attribute transitions between two modifier sets, in the exact
/// order crossterm produces: removals first, then the intensity reset with
/// its re-applications, then additions.
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
            buffer.content.iter().enumerate().map(|(index, cell)| {
                ((index % width) as u16, (index / width) as u16, cell)
            }),
        )
        .unwrap();
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
            "\x1b[1;1H \x1b[39m\x1b[49m\x1b[0m"
        );
        // A reset-colored cell is indistinguishable from the default state.
        let mut cell = ratatui::buffer::Cell::default();
        cell.set_symbol("x");
        assert_eq!(draw_cell(cell), "\x1b[1;1Hx\x1b[39m\x1b[49m\x1b[0m");
    }

    #[test]
    fn contiguous_cells_skip_the_move() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 3, 1));
        buffer.set_string(0, 0, "abc", Style::default());
        let output = String::from_utf8(draw(&buffer)).unwrap();
        assert_eq!(output, "\x1b[1;1Habc\x1b[39m\x1b[49m\x1b[0m");
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
            "\x1b[1;1H\x1b[1m\x1b[31;44mx\x1b[39m\x1b[49m\x1b[0m"
        );
    }

    #[test]
    fn color_parameters_match_the_crossterm_table() {
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
                format!("\x1b[1;1H\x1b[{foreground};{background}mx\x1b[39m\x1b[49m\x1b[0m"),
                "color {color:?}"
            );
        }
    }

    #[test]
    fn modifier_diff_matches_crossterm_ordering() {
        // Intensity reset re-applies the surviving half.
        assert_eq!(
            diff(Modifier::BOLD, Modifier::DIM),
            "\x1b[22m\x1b[2m"
        );
        assert_eq!(
            diff(Modifier::HIDDEN | Modifier::DIM, Modifier::BOLD | Modifier::DIM),
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
            assert_eq!(String::from_utf8(backend.writer).unwrap(), expected);
        }
    }

    #[test]
    fn append_lines_writes_newlines() {
        let mut backend = FshellBackend::new(Vec::new());
        backend.append_lines(2).unwrap();
        assert_eq!(backend.writer, b"\n\n");
    }
}
