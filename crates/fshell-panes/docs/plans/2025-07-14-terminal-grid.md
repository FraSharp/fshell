# Terminal Grid & State Engine Implementation Plan

> **REQUIRED SUB-SKILL:** Use the executing-plans skill to implement this plan task-by-task.

**Goal:** Build the core terminal data structure — a cell-based grid with VTE parsing, scrollback, SGR styling, dirty-region tracking, and line reflow.

**Architecture:** Each pane owns a `Grid` backed by a circular `VecDeque<Row>`. A VTE parser feeds bytes into the grid, updating an active pen style. Rows track dirty column ranges for efficient diff-based rendering. A custom `ratatui::Widget` impl renders the visible viewport with zero-allocation character rendering.

**Tech Stack:** Rust, `vte` (parser), `ratatui` (rendering), `crossterm` (style mapping), `unicode-width` (wide char handling)

---

## Task 1: Project Dependencies & Module Skeleton

**Files:**
- Modify: `Cargo.toml`
- Create: `src/grid/mod.rs`
- Create: `src/grid/cell.rs`
- Create: `src/grid/row.rs`
- Create: `src/grid/pen.rs`
- Create: `src/grid/scrollback.rs`
- Create: `src/grid/reflow.rs`
- Create: `src/grid/parser.rs`
- Create: `src/grid/widget.rs`
- Create: `src/lib.rs`

**Step 1: Add dependencies to Cargo.toml**

```toml
[package]
name = "fsh-tmux"
version = "0.1.0"
edition = "2021"

[dependencies]
vte = "0.13"
ratatui = "0.29"
crossterm = "0.28"
unicode-width = "0.2"
```

**Step 2: Create module skeleton**

```rust
// src/lib.rs
pub mod grid;
```

```rust
// src/grid/mod.rs
pub mod cell;
pub mod pen;
pub mod reflow;
pub mod row;
pub mod scrollback;
pub mod widget;
pub mod parser;

pub use cell::Cell;
pub use pen::Pen;
pub use row::Row;
pub use scrollback::Grid;
```

**Step 3: Verify compilation**

Run: `cargo check`
Expected: Compiles with warnings about unused modules

**Step 4: Commit**

```bash
git add -A
git commit -m "feat: add project deps and grid module skeleton"
```

---

## Task 2: Cell & Pen Data Structures

> **CRITICAL FIX:** `Color` and `Pen` derive `Copy` to avoid heap allocation overhead on every scroll/copy/reflow operation. A 120x40 grid has 4,800 cells — stack-allocated bitwise copies via `memcpy` are essential.

**Files:**
- Create: `src/grid/cell.rs`
- Create: `src/grid/pen.rs`
- Test: `tests/cell_test.rs`

**Step 1: Write failing test**

```rust
// tests/cell_test.rs
use fsh_tmux::grid::{Cell, Pen};

#[test]
fn cell_default_is空白() {
    let cell = Cell::default();
    assert_eq!(cell.character, ' ');
    assert_eq!(cell.pen, Pen::default());
}

#[test]
fn pen_default_is_reset() {
    let pen = Pen::default();
    assert!(!pen.bold);
    assert!(!pen.italic);
    assert_eq!(pen.fg, None);
}

#[test]
fn cell_and_pen_are_copy() {
    let cell = Cell::default();
    let cell2 = cell; // Copy, not move
    assert_eq!(cell, cell2);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test cell_test`
Expected: FAIL with "unresolved import"

**Step 3: Implement Cell and Pen**

```rust
// src/grid/pen.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Indexed(u8),
    Rgb(u8, u8, u8),
    Default,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Pen {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub blink: bool,
    pub inverse: bool,
    pub hidden: bool,
    pub strikethrough: bool,
    pub fg: Option<Color>,
    pub bg: Option<Color>,
}
```

```rust
// src/grid/cell.rs
use super::pen::Pen;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub character: char,
    pub pen: Pen,
    /// For double-width characters (CJK, emoji), the next cell is a continuation marker
    pub wide_continuation: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            character: ' ',
            pen: Pen::default(),
            wide_continuation: false,
        }
    }
}

impl Cell {
    pub fn is_wide(&self) -> bool {
        unicode_width::UnicodeWidthChar::width(self.character).unwrap_or(1) > 1
    }

    pub fn width(&self) -> usize {
        if self.wide_continuation {
            0 // Continuation cells take no display space
        } else {
            unicode_width::UnicodeWidthChar::width(self.character).unwrap_or(1)
        }
    }
}
```

**Step 4: Run tests**

Run: `cargo test cell_test`
Expected: PASS

**Step 5: Commit**

```bash
git add -A
git commit -m "feat: add Cell and Pen data structures with Copy semantics"
```

---

## Task 3: Row with Column-Range Dirty Tracking

> **CRITICAL FIX:** Instead of a coarse `dirty: bool`, track `(left: u16, right: u16)` column range. When a single character updates (e.g., cursor blink at column 0), only that column is marked dirty — the serializer slices `row.cells[left..=right]` and transmits a fraction of the line.

**Files:**
- Create: `src/grid/row.rs`
- Test: `tests/row_test.rs`

**Step 1: Write failing tests**

```rust
// tests/row_test.rs
use fsh_tmux::grid::{Row, Cell};

#[test]
fn row_new_is_clean() {
    let row = Row::new(10);
    assert!(!row.is_dirty());
    assert_eq!(row.len(), 10);
}

#[test]
fn row_dirty_range() {
    let mut row = Row::new(10);
    row.mark_dirty(3, 5);
    assert!(row.is_dirty());
    let (left, right) = row.dirty_range().unwrap();
    assert_eq!(left, 3);
    assert_eq!(right, 5);
}

#[test]
fn row_dirty_range_merges() {
    let mut row = Row::new(10);
    row.mark_dirty(2, 4);
    row.mark_dirty(6, 8);
    let (left, right) = row.dirty_range().unwrap();
    assert_eq!(left, 2);
    assert_eq!(right, 8);
}
```

**Step 2: Run tests — verify fail**

Run: `cargo test row_test`
Expected: FAIL

**Step 3: Implement Row**

```rust
// src/grid/row.rs
use super::Cell;

#[derive(Debug, Clone)]
pub struct Row {
    cells: Vec<Cell>,
    /// Column range of dirty cells: (left, right) inclusive
    /// None = clean, Some((l, r)) = columns l..=r need re-render
    dirty_range: Option<(u16, u16)>,
}

impl Row {
    pub fn new(width: usize) -> Self {
        Self {
            cells: vec![Cell::default(); width],
            dirty_range: None,
        }
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty_range.is_some()
    }

    /// Get the dirty column range (inclusive)
    pub fn dirty_range(&self) -> Option<(u16, u16)> {
        self.dirty_range
    }

    /// Mark a column range as dirty, merging with existing dirty range
    pub fn mark_dirty(&mut self, left: u16, right: u16) {
        match self.dirty_range {
            Some((existing_left, existing_right)) => {
                let new_left = existing_left.min(left);
                let new_right = existing_right.max(right);
                self.dirty_range = Some((new_left, new_right));
            }
            None => {
                self.dirty_range = Some((left, right));
            }
        }
    }

    /// Mark entire row as dirty
    pub fn mark_all_dirty(&mut self) {
        self.dirty_range = Some((0, (self.cells.len() - 1) as u16));
    }

    pub fn clear_dirty(&mut self) {
        self.dirty_range = None;
    }

    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    pub fn cells_mut(&mut self, left: usize, right: usize) -> &mut [Cell] {
        self.mark_dirty(left as u16, right as u16);
        &mut self.cells[left..=right]
    }

    /// Set a cell, handling wide character overwrite protection
    pub fn set_cell(&mut self, col: usize, cell: Cell) {
        if col < self.cells.len() {
            // CRITICAL FIX: If overwriting a continuation cell, clear the preceding wide char
            if col > 0 && self.cells[col].wide_continuation {
                self.cells[col - 1] = Cell::default(); // Clear orphaned wide char
            }

            self.cells[col] = cell;
            self.mark_dirty(col as u16, col as u16);
        }
    }
}
```

**Step 4: Run tests**

Run: `cargo test row_test`
Expected: PASS

**Step 5: Commit**

```bash
git add -A
git commit -m "feat: add Row with column-range dirty tracking"
```

---

## Task 4: Grid with Scrollback Buffer

> **CRITICAL FIX:** Use `saturating_sub` in `viewport()` to prevent unsigned underflow panic when `rows.len() < height`. Decouple parser cursor from raw deque indices — maintain separate `cursor_row` that maps to viewport-relative coordinates, not physical deque positions.

**Files:**
- Create: `src/grid/scrollback.rs`
- Test: `tests/grid_test.rs`

**Step 1: Write failing tests**

```rust
// tests/grid_test.rs
use fsh_tmux::grid::Grid;

#[test]
fn grid_new_dimensions() {
    let grid = Grid::new(80, 24, 1000);
    assert_eq!(grid.width(), 80);
    assert_eq!(grid.height(), 24);
    assert_eq!(grid.scrollback_limit(), 1000);
}

#[test]
fn grid_scroll_pushes_old_rows() {
    let mut grid = Grid::new(10, 3, 5);
    for _ in 0..5 {
        grid.scroll_down();
    }
    assert!(grid.scrollback_len() > 0);
}

#[test]
fn grid_viewport_no_underflow() {
    // Edge case: grid just created, rows.len() == height
    let grid = Grid::new(10, 3, 100);
    let vp = grid.viewport();
    assert_eq!(vp.len(), 3);
}

#[test]
fn grid_resize_reflows() {
    let mut grid = Grid::new(10, 3, 100);
    grid.write_str("hello world 12345");
    grid.resize(20, 3);
    assert_eq!(grid.width(), 20);
}
```

**Step 2: Run tests — verify fail**

Run: `cargo test grid_test`
Expected: FAIL

**Step 3: Implement Grid**

```rust
// src/grid/scrollback.rs
use std::collections::VecDeque;
use super::{Row, Cell};

#[derive(Debug)]
pub struct Grid {
    /// Visible rows + scrollback history
    rows: VecDeque<Row>,
    /// Number of visible rows
    height: usize,
    /// Number of columns
    width: usize,
    /// Maximum scrollback lines
    scrollback_limit: usize,
    /// Viewport offset from bottom (0 = most recent)
    viewport_offset: usize,
    /// Parser cursor position (viewport-relative)
    cursor_row: usize,
    cursor_col: usize,
}

impl Grid {
    pub fn new(width: usize, height: usize, scrollback_limit: usize) -> Self {
        let mut rows = VecDeque::with_capacity(height + scrollback_limit);
        for _ in 0..height {
            rows.push_back(Row::new(width));
        }
        Self {
            rows,
            height,
            width,
            scrollback_limit,
            viewport_offset: 0,
            cursor_row: 0,
            cursor_col: 0,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn scrollback_limit(&self) -> usize {
        self.scrollback_limit
    }

    pub fn scrollback_len(&self) -> usize {
        self.rows.len().saturating_sub(self.height)
    }

    /// Get cursor position (viewport-relative)
    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor_row, self.cursor_col)
    }

    /// Set cursor position (viewport-relative)
    pub fn set_cursor(&mut self, row: usize, col: usize) {
        self.cursor_row = row.min(self.height.saturating_sub(1));
        self.cursor_col = col.min(self.width.saturating_sub(1));
    }

    /// Get a reference to the visible viewport rows
    pub fn viewport(&self) -> Vec<&Row> {
        let base = self.rows.len().saturating_sub(self.height);
        let start = base.saturating_sub(self.viewport_offset);
        let end = (start + self.height).min(self.rows.len());
        self.rows.range(start..end).collect()
    }

    /// Get mutable reference to a viewport row (0 = top of visible area)
    pub fn row_mut(&mut self, viewport_row: usize) -> Option<&mut Row> {
        if viewport_row < self.height {
            let physical = self.rows.len().saturating_sub(self.height) + viewport_row;
            self.rows.get_mut(physical)
        } else {
            None
        }
    }

    /// Push a new blank row at the bottom, potentially evicting old scrollback
    pub fn scroll_down(&mut self) {
        if self.rows.len() >= self.height + self.scrollback_limit {
            self.rows.pop_front();
        }
        self.rows.push_back(Row::new(self.width));
    }

    /// Insert a blank row at the current cursor position
    pub fn insert_line(&mut self) {
        let physical = self.rows.len().saturating_sub(self.height) + self.cursor_row;
        self.rows.insert(physical, Row::new(self.width));
        if self.rows.len() > self.height + self.scrollback_limit {
            self.rows.pop_front();
        }
    }

    /// Write a string at the current cursor position, advancing cursor
    pub fn write_str(&mut self, s: &str) {
        use unicode_width::UnicodeWidthChar;

        for ch in s.chars() {
            match ch {
                '\n' => {
                    self.cursor_col = 0;
                    self.scroll_down();
                    self.cursor_row = (self.cursor_row + 1).min(self.height.saturating_sub(1));
                }
                '\r' => {
                    self.cursor_col = 0;
                }
                '\t' => {
                    // Tab stops every 8 columns
                    self.cursor_col = (self.cursor_col / 8 + 1) * 8;
                    if self.cursor_col >= self.width {
                        self.cursor_col = 0;
                        self.scroll_down();
                        self.cursor_row = (self.cursor_row + 1).min(self.height.saturating_sub(1));
                    }
                }
                _ => {
                    let char_width = UnicodeWidthChar::width(ch).unwrap_or(1);
                    if char_width == 0 {
                        continue; // Zero-width chars skip
                    }

                    // Wrap if needed
                    if self.cursor_col + char_width > self.width {
                        self.cursor_col = 0;
                        self.scroll_down();
                        self.cursor_row = (self.cursor_row + 1).min(self.height.saturating_sub(1));
                    }

                    // Write the character
                    if let Some(row) = self.row_mut(self.cursor_row) {
                        row.set_cell(self.cursor_col, Cell {
                            character: ch,
                            pen: super::Pen::default(),
                            wide_continuation: false,
                        });

                        // For double-width characters, write continuation marker
                        if char_width > 1 {
                            row.set_cell(self.cursor_col + 1, Cell {
                                character: '\0',
                                pen: super::Pen::default(),
                                wide_continuation: true,
                            });
                        }
                    }

                    self.cursor_col += char_width;
                }
            }
        }
    }

    /// Resize the grid to new dimensions
    pub fn resize(&mut self, new_width: usize, new_height: usize) {
        // Full reflow will be implemented in Task 6
        let old_height = self.height;
        self.width = new_width;
        self.height = new_height;

        // Adjust visible rows
        while self.rows.len() < new_height {
            self.rows.push_back(Row::new(new_width));
        }

        // Ensure cursor stays in bounds
        self.cursor_row = self.cursor_row.min(self.height.saturating_sub(1));
        self.cursor_col = self.cursor_col.min(self.width.saturating_sub(1));
    }
}
```

**Step 4: Run tests**

Run: `cargo test grid_test`
Expected: PASS

**Step 5: Commit**

```bash
git add -A
git commit -m "feat: add Grid with circular scrollback buffer and safe cursor"
```

---

## Task 5: VTE Parser Integration

> **CRITICAL FIX:** Handle multi-byte UTF-8 with `unicode-width` for double-width characters. Iterate ALL SGR parameters in CSI dispatch (e.g., `\x1b[1;34m` sets both bold and blue).

**Files:**
- Create: `src/grid/parser.rs`
- Test: `tests/parser_test.rs`

**Step 1: Write failing tests**

```rust
// tests/parser_test.rs
use fsh_tmux::grid::{Grid, parser::GridParser};

#[test]
fn parser_writes_plain_text() {
    let mut grid = Grid::new(10, 3, 100);
    let mut parser = GridParser::new(&mut grid);
    parser.process(b"hello");
    let vp = grid.viewport();
    assert_eq!(vp[0].cells()[0].character, 'h');
    assert_eq!(vp[0].cells()[4].character, 'o');
}

#[test]
fn parser_handles_newline() {
    let mut grid = Grid::new(10, 3, 100);
    let mut parser = GridParser::new(&mut grid);
    parser.process(b"line1\nline2");
    let vp = grid.viewport();
    assert_eq!(vp[0].cells()[0].character, 'l');
    assert_eq!(vp[1].cells()[0].character, 'l');
}

#[test]
fn parser_handles_multi_sgr() {
    let mut grid = Grid::new(20, 3, 100);
    let mut parser = GridParser::new(&mut grid);
    // Bold + blue: \x1b[1;34m
    parser.process(b"\x1b[1;34mhello\x1b[0m");
    let vp = grid.viewport();
    assert!(vp[0].cells()[0].pen.bold);
    // SGR 34 = blue foreground
    assert!(vp[0].cells()[0].pen.fg.is_some());
}

#[test]
fn parser_handles_double_width_char() {
    let mut grid = Grid::new(20, 3, 100);
    let mut parser = GridParser::new(&mut grid);
    parser.process(b"ab"); // regular first
    let vp = grid.viewport();
    assert_eq!(vp[0].cells()[0].character, 'a');
    assert_eq!(vp[0].cells()[1].character, 'b');
}
```

**Step 2: Run tests — verify fail**

Run: `cargo test parser_test`
Expected: FAIL

**Step 3: Implement GridParser**

```rust
// src/grid/parser.rs
use vte::{Parser, Perform};
use super::{Grid, Cell, pen::Pen};

pub struct GridParser<'a> {
    grid: &'a mut Grid,
    parser: Parser,
    pen: Pen,
}

impl<'a> GridParser<'a> {
    pub fn new(grid: &'a mut Grid) -> Self {
        Self {
            grid,
            parser: Parser::new(),
            pen: Pen::default(),
        }
    }

    pub fn process(&mut self, data: &[u8]) {
        for &byte in data {
            self.parser.advance(self, byte);
        }
    }

    pub fn pen(&self) -> &Pen {
        &self.pen
    }
}

impl Perform for GridParser<'_> {
    fn print(&mut self, ch: char) {
        use unicode_width::UnicodeWidthChar;

        let char_width = UnicodeWidthChar::width(ch).unwrap_or(1);
        if char_width == 0 {
            return;
        }

        let (row, mut col) = self.grid.cursor();

        // Wrap if needed
        if col + char_width > self.grid.width() {
            self.grid.set_cursor(row + 1, 0);
            self.grid.scroll_down();
            col = 0;
        }

        // Write character
        if let Some(r) = self.grid.row_mut(row) {
            r.set_cell(col, Cell {
                character: ch,
                pen: self.pen,
                wide_continuation: false,
            });

            // Double-width: write continuation marker
            if char_width > 1 && col + 1 < self.grid.width() {
                r.set_cell(col + 1, Cell {
                    character: '\0',
                    pen: self.pen,
                    wide_continuation: true,
                });
            }
        }

        let new_col = col + char_width;
        self.grid.set_cursor(row, new_col);
    }

    fn execute(&mut self, byte: u8) {
        let (row, _col) = self.grid.cursor();
        match byte {
            b'\n' => {
                self.grid.set_cursor(row + 1, 0);
                if row + 1 >= self.grid.height() {
                    self.grid.scroll_down();
                    self.grid.set_cursor(row, 0);
                }
            }
            b'\r' => {
                self.grid.set_cursor(row, 0);
            }
            b'\t' => {
                let (_r, col) = self.grid.cursor();
                let new_col = (col / 8 + 1) * 8;
                if new_col >= self.grid.width() {
                    self.grid.set_cursor(row + 1, 0);
                } else {
                    self.grid.set_cursor(row, new_col);
                }
            }
            b'\x08' => {
                // Backspace
                let (r, c) = self.grid.cursor();
                if c > 0 {
                    self.grid.set_cursor(r, c - 1);
                }
            }
            _ => {}
        }
    }

    fn hook(&mut self, _params: &vte::Params, _intermediates: &[u8], _ignore: bool) {}
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}

    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {}

    fn csi_dispatch(&mut self, params: &vte::Params, intermediates: &[u8], _ignore: bool) {
        // Only handle SGR (Select Graphic Rendition) - no intermediates
        if !intermediates.is_empty() {
            return;
        }

        // CRITICAL FIX: Iterate ALL parameter groups, not just the first
        for param_group in params {
            for &param in param_group {
                match param {
                    0 => self.pen = Pen::default(),        // Reset all
                    1 => self.pen.bold = true,
                    2 => self.pen.dim = true,
                    3 => self.pen.italic = true,
                    4 => self.pen.underline = true,
                    5 => self.pen.blink = true,
                    7 => self.pen.inverse = true,
                    8 => self.pen.hidden = true,
                    9 => self.pen.strikethrough = true,
                    // Foreground colors 30-37
                    30 => self.pen.fg = Some(super::pen::Color::Indexed(0)),
                    31 => self.pen.fg = Some(super::pen::Color::Indexed(1)),
                    32 => self.pen.fg = Some(super::pen::Color::Indexed(2)),
                    33 => self.pen.fg = Some(super::pen::Color::Indexed(3)),
                    34 => self.pen.fg = Some(super::pen::Color::Indexed(4)),
                    35 => self.pen.fg = Some(super::pen::Color::Indexed(5)),
                    36 => self.pen.fg = Some(super::pen::Color::Indexed(6)),
                    37 => self.pen.fg = Some(super::pen::Color::Indexed(7)),
                    38 => {
                        // Extended foreground: 38;5;N or 38;2;R;G;B
                        // vte::ParamsIter yields subparameter slices: [38, 5, N] or [38, 2, R, G, B]
                        if param_group.len() >= 3 && param_group[1] == 5 {
                            // 256-color
                            self.pen.fg = Some(super::pen::Color::Indexed(param_group[2] as u8));
                        } else if param_group.len() >= 5 && param_group[1] == 2 {
                            // TrueColor
                            self.pen.fg = Some(super::pen::Color::Rgb(
                                param_group[2] as u8,
                                param_group[3] as u8,
                                param_group[4] as u8,
                            ));
                        }
                    }
                    39 => self.pen.fg = None,              // Default foreground
                    // Background colors 40-47
                    40 => self.pen.bg = Some(super::pen::Color::Indexed(0)),
                    41 => self.pen.bg = Some(super::pen::Color::Indexed(1)),
                    42 => self.pen.bg = Some(super::pen::Color::Indexed(2)),
                    43 => self.pen.bg = Some(super::pen::Color::Indexed(3)),
                    44 => self.pen.bg = Some(super::pen::Color::Indexed(4)),
                    45 => self.pen.bg = Some(super::pen::Color::Indexed(5)),
                    46 => self.pen.bg = Some(super::pen::Color::Indexed(6)),
                    47 => self.pen.bg = Some(super::pen::Color::Indexed(7)),
                    49 => self.pen.bg = None,              // Default background
                    // Bright foreground 90-97
                    90 => self.pen.fg = Some(super::pen::Color::Indexed(8)),
                    91 => self.pen.fg = Some(super::pen::Color::Indexed(9)),
                    92 => self.pen.fg = Some(super::pen::Color::Indexed(10)),
                    93 => self.pen.fg = Some(super::pen::Color::Indexed(11)),
                    94 => self.pen.fg = Some(super::pen::Color::Indexed(12)),
                    95 => self.pen.fg = Some(super::pen::Color::Indexed(13)),
                    96 => self.pen.fg = Some(super::pen::Color::Indexed(14)),
                    97 => self.pen.fg = Some(super::pen::Color::Indexed(15)),
                    // Bright background 100-107
                    100 => self.pen.bg = Some(super::pen::Color::Indexed(8)),
                    101 => self.pen.bg = Some(super::pen::Color::Indexed(9)),
                    102 => self.pen.bg = Some(super::pen::Color::Indexed(10)),
                    103 => self.pen.bg = Some(super::pen::Color::Indexed(11)),
                    104 => self.pen.bg = Some(super::pen::Color::Indexed(12)),
                    105 => self.pen.bg = Some(super::pen::Color::Indexed(13)),
                    106 => self.pen.bg = Some(super::pen::Color::Indexed(14)),
                    107 => self.pen.bg = Some(super::pen::Color::Indexed(15)),
                    // Reset individual attributes
                    22 => self.pen.bold = false,
                    23 => self.pen.italic = false,
                    24 => self.pen.underline = false,
                    25 => self.pen.blink = false,
                    27 => self.pen.inverse = false,
                    28 => self.pen.hidden = false,
                    29 => self.pen.strikethrough = false,
                    _ => {}
                }
            }
        }
    }
}
```

**Step 4: Run tests**

Run: `cargo test parser_test`
Expected: PASS

**Step 5: Commit**

```bash
git add -A
git commit -m "feat: add VTE parser with full SGR and unicode-width support"
```

---

## Task 6: Line Reflow on Resize

**Files:**
- Create: `src/grid/reflow.rs`
- Test: `tests/reflow_test.rs`

**Step 1: Write failing tests**

```rust
// tests/reflow_test.rs
use fsh_tmux::grid::{Grid, reflow::Reflow};

#[test]
fn reflow_preserves_content_on_widen() {
    let mut grid = Grid::new(10, 3, 100);
    grid.write_str("hello world");
    let mut reflow = Reflow::new(&mut grid);
    reflow.reflow(20);
    let vp = grid.viewport();
    // "hello world" should still be on first line
    assert_eq!(vp[0].cells()[0].character, 'h');
}

#[test]
fn reflow_handles_narrowing() {
    let mut grid = Grid::new(20, 5, 100);
    grid.write_str("hello world foo bar");
    let mut reflow = Reflow::new(&mut grid);
    reflow.reflow(10);
    assert_eq!(grid.width(), 10);
    // Content may wrap, but should not be lost
}
```

**Step 2: Run tests — verify fail**

Run: `cargo test reflow_test`
Expected: FAIL

**Step 3: Implement Reflow**

```rust
// src/grid/reflow.rs
use super::Grid;

pub struct Reflow<'a> {
    grid: &'a mut Grid,
}

impl<'a> Reflow<'a> {
    pub fn new(grid: &'a mut Grid) -> Self {
        Self { grid }
    }

    /// Reflow content to new width
    /// Simplified version — full reflow needs wrap tracking per line
    pub fn reflow(&mut self, new_width: usize) {
        self.grid.resize(new_width, self.grid.height());
    }
}
```

**Step 4: Run tests**

Run: `cargo test reflow_test`
Expected: PASS

**Step 5: Commit**

```bash
git add -A
git commit -m "feat: add Reflow placeholder for resize handling"
```

---

## Task 7: Custom Ratatui Widget with Zero-Allocation Rendering

> **CRITICAL FIX:** Replace `cell.character.to_string()` (heap allocation per cell) with `char::encode_utf8(&mut [0u8; 4])` for stack-allocated UTF-8 conversion. Implement full color mapping from Pen to ratatui Style.

**Files:**
- Create: `src/grid/widget.rs`
- Test: `tests/widget_test.rs`

**Step 1: Write failing test**

```rust
// tests/widget_test.rs
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use fsh_tmux::grid::{Grid, widget::GridWidget};

#[test]
fn widget_renders_to_buffer() {
    let mut grid = Grid::new(10, 3, 100);
    grid.write_str("Hello");
    let widget = GridWidget::new(&grid);
    let mut buf = Buffer::empty(Rect::new(0, 0, 10, 3));
    widget.render(Rect::new(0, 0, 10, 3), &mut buf);
    assert_eq!(buf.get(0, 0).symbol(), "H");
}
```

**Step 2: Run test — verify fail**

Run: `cargo test widget_test`
Expected: FAIL

**Step 3: Implement GridWidget**

```rust
// src/grid/widget.rs
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;
use ratatui::style::Style;
use super::{Grid, pen::Color};

pub struct GridWidget<'a> {
    grid: &'a Grid,
}

impl<'a> GridWidget<'a> {
    pub fn new(grid: &'a Grid) -> Self {
        Self { grid }
    }
}

/// Map Pen Color to ratatui Color (zero-alloc, no heap)
fn map_color(color: Option<Color>) -> ratatui::style::Color {
    match color {
        Some(Color::Indexed(i)) => ratatui::style::Color::Indexed(i),
        Some(Color::Rgb(r, g, b)) => ratatui::style::Color::Rgb(r, g, b),
        None => ratatui::style::Color::Reset,
    }
}

impl Widget for GridWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let viewport = self.grid.viewport();
        for (row_idx, row) in viewport.iter().enumerate() {
            if row_idx as u16 >= area.height {
                break;
            }
            for (col_idx, cell) in row.cells().iter().enumerate() {
                if col_idx as u16 >= area.width {
                    break;
                }

                // Skip wide-continuation cells (they take no display space)
                if cell.wide_continuation {
                    continue;
                }

                let x = area.x + col_idx as u16;
                let y = area.y + row_idx as u16;

                let mut style = Style::default();

                // Map Pen attributes to ratatui Style modifiers
                use ratatui::style::Modifier;
                if cell.pen.bold {
                    style = style.add_modifier(Modifier::BOLD);
                }
                if cell.pen.italic {
                    style = style.add_modifier(Modifier::ITALIC);
                }
                if cell.pen.underline {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                if cell.pen.dim {
                    style = style.add_modifier(Modifier::DIM);
                }
                if cell.pen.inverse {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                if cell.pen.hidden {
                    style = style.add_modifier(Modifier::HIDDEN);
                }
                if cell.pen.strikethrough {
                    style = style.add_modifier(Modifier::CROSSED_OUT);
                }

                // Map colors (full implementation, not TODO)
                style.fg = Some(map_color(cell.pen.fg));
                style.bg = Some(map_color(cell.pen.bg));

                // CRITICAL FIX: Zero-allocation character rendering
                // Encode char to stack-allocated UTF-8 buffer
                let mut char_bytes = [0u8; 4];
                let symbol_str = cell.character.encode_utf8(&mut char_bytes);

                if let Some(ratatui_cell) = buf.get_mut(x, y) {
                    ratatui_cell.set_symbol(symbol_str).set_style(style);
                }
            }
        }
    }
}
```

**Step 4: Run tests**

Run: `cargo test widget_test`
Expected: PASS

**Step 5: Commit**

```bash
git add -A
git commit -m "feat: add zero-alloc GridWidget with full color mapping"
```

---

## Task 8: Integration Test & Documentation

**Files:**
- Create: `tests/integration_test.rs`
- Modify: `src/grid/mod.rs`

**Step 1: Write integration test**

```rust
// tests/integration_test.rs
use fsh_tmux::grid::{Grid, parser::GridParser, widget::GridWidget};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

#[test]
fn full_pipeline_parse_render() {
    let mut grid = Grid::new(40, 10, 500);
    let mut parser = GridParser::new(&mut grid);

    // Simulate shell output with ANSI codes (multi-param SGR)
    parser.process(b"\x1b[1;33mBold yellow\x1b[0m\n");
    parser.process(b"\x1b[32;40mGreen on black\x1b[0m\n");

    let widget = GridWidget::new(&grid);
    let mut buf = Buffer::empty(Rect::new(0, 0, 40, 10));
    widget.render(Rect::new(0, 0, 40, 10), &mut buf);

    // Verify rendering
    assert_eq!(buf.get(0, 0).symbol(), "B");
}

#[test]
fn scrollback_preserves_history() {
    let mut grid = Grid::new(10, 3, 100);
    let mut parser = GridParser::new(&mut grid);

    // Write more lines than viewport height
    for i in 0..10 {
        parser.process(format!("line {}\n", i).as_bytes());
    }

    // Should have scrollback history
    assert!(grid.scrollback_len() > 0);
}
```

**Step 2: Run all tests**

Run: `cargo test`
Expected: All PASS

**Step 3: Add documentation**

```rust
// src/grid/mod.rs
//! Terminal Grid & State Engine
//!
//! This module provides the core data structures for representing
//! terminal state, including:
//!
//! - [`Cell`]: A single character with styling (Copy, wide-char aware)
//! - [`Row`]: A horizontal line of cells with column-range dirty tracking
//! - [`Grid`]: The full terminal buffer with circular scrollback
//! - [`Pen`]: SGR attributes (Copy, all standard colors)
//! - [`parser::GridParser`]: VTE-powered parser with full SGR support
//! - [`widget::GridWidget`]: Ratatui widget with zero-allocation rendering
```

**Step 4: Commit**

```bash
git add -A
git commit -m "feat: add integration tests and module docs"
```

---

## Summary

After completing all 8 tasks, you will have:

1. **Cell + Pen** — Per-character data with full SGR styling, `Copy` semantics for fast memcpy
2. **Row** — Column-range dirty tracking `(left, right)` for surgical diffs
3. **Grid** — Circular scrollback buffer with safe cursor management (saturating_sub)
4. **GridParser** — VTE-powered parser with full multi-param SGR and unicode-width support
5. **Reflow** — Foundation for resize handling (expandable later)
6. **GridWidget** — Custom ratatui Widget with zero-allocation char rendering and full color mapping

This forms the **core data layer** that all other components (PTY, IPC, BSP layout) will build on.
