// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

//! Interactive DataGrid component for structured pipeline records with sorting, filtering, and scrolling.

use crate::tui::theme;
use fshell_core::Val;
use fshell_core::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Cell, Row, Table, TableState};
use unicode_width::UnicodeWidthStr;
use ustr::ustr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

impl SortDirection {
    pub fn toggle(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            Self::Ascending => "▲",
            Self::Descending => "▼",
        }
    }
}

/// State for the DataGrid table explorer.
#[derive(Debug, Clone, Default)]
pub struct DataGridState {
    pub table_state: TableState,
    pub sort_column: Option<(String, SortDirection)>,
    pub filter_query: String,
    pub active_column_idx: usize,
    pub col_offset: usize,
    pub cached_indices: Option<Vec<usize>>,
    pub cached_widths: Option<Vec<usize>>,
    pub cached_is_numeric: Option<Vec<bool>>,
    last_query: String,
    last_sort: Option<(String, SortDirection)>,
    last_item_count: usize,
}

impl DataGridState {
    pub fn new() -> Self {
        let mut s = Self::default();
        s.table_state.select(Some(0));
        s
    }

    pub fn ensure_indices(&mut self, items: &[Val]) -> &[usize] {
        if self.cached_indices.is_none()
            || self.last_query != self.filter_query
            || self.last_sort != self.sort_column
            || self.last_item_count != items.len()
        {
            let indices = DataGrid::filter_and_sort_indices(
                items,
                &self.filter_query,
                self.sort_column.as_ref(),
            );
            self.cached_indices = Some(indices);
            self.cached_widths = None;
            self.cached_is_numeric = None;
            self.last_query = self.filter_query.clone();
            self.last_sort = self.sort_column.clone();
            self.last_item_count = items.len();
        }
        self.cached_indices.as_deref().unwrap_or(&[])
    }

    pub fn selected(&self) -> usize {
        self.table_state.selected().unwrap_or(0)
    }

    pub fn select_next(&mut self, total_rows: usize) {
        if total_rows == 0 {
            self.table_state.select(None);
            return;
        }
        let current = self.selected();
        let next = if current + 1 < total_rows {
            current + 1
        } else {
            0
        };
        self.table_state.select(Some(next));
    }

    pub fn select_prev(&mut self, total_rows: usize) {
        if total_rows == 0 {
            self.table_state.select(None);
            return;
        }
        let current = self.selected();
        let prev = if current > 0 {
            current - 1
        } else {
            total_rows - 1
        };
        self.table_state.select(Some(prev));
    }

    pub fn page_down(&mut self, total_rows: usize, page: usize) {
        if total_rows == 0 {
            return;
        }
        let current = self.selected();
        let next = (current + page).min(total_rows.saturating_sub(1));
        self.table_state.select(Some(next));
    }

    pub fn page_up(&mut self, page: usize) {
        let current = self.selected();
        let prev = current.saturating_sub(page);
        self.table_state.select(Some(prev));
    }

    pub fn next_column(&mut self, total_cols: usize) {
        if total_cols == 0 {
            self.active_column_idx = 0;
            return;
        }
        if self.active_column_idx + 1 < total_cols {
            self.active_column_idx += 1;
        } else {
            self.active_column_idx = 0;
        }
    }

    pub fn prev_column(&mut self, total_cols: usize) {
        if total_cols == 0 {
            self.active_column_idx = 0;
            return;
        }
        if self.active_column_idx > 0 {
            self.active_column_idx -= 1;
        } else {
            self.active_column_idx = total_cols - 1;
        }
    }

    pub fn toggle_active_sort(&mut self, columns: &[String]) {
        if columns.is_empty() {
            return;
        }
        let idx = self.active_column_idx.min(columns.len() - 1);
        let col = &columns[idx];
        self.toggle_sort(col);
    }

    pub fn toggle_sort(&mut self, col_name: &str) {
        let new_dir = match &self.sort_column {
            Some((name, dir)) if name == col_name => dir.toggle(),
            _ => SortDirection::Ascending,
        };
        self.sort_column = Some((col_name.to_string(), new_dir));
    }
}

/// DataGrid view component.
pub struct DataGrid<'a> {
    pub items: &'a [Val],
    pub theme: &'a Theme,
    pub state: &'a mut DataGridState,
}

impl<'a> DataGrid<'a> {
    pub fn new(items: &'a [Val], theme: &'a Theme, state: &'a mut DataGridState) -> Self {
        Self {
            items,
            theme,
            state,
        }
    }

    /// Collect all unique column keys from map items.
    pub fn extract_columns(items: &[Val]) -> Vec<String> {
        let mut keys = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for item in items {
            if let Val::Map(map) = item {
                for (k, _) in map {
                    if seen.insert(k.as_str()) {
                        keys.push(k.to_string());
                    }
                }
            }
        }
        keys
    }

    /// Filter and sort row indices according to query and sort specification.
    pub fn filter_and_sort_indices(
        items: &[Val],
        query: &str,
        sort: Option<&(String, SortDirection)>,
    ) -> Vec<usize> {
        let q = query.trim().to_lowercase();
        let mut indices: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                if q.is_empty() {
                    return true;
                }
                if let Val::Map(map) = item {
                    for (_, v) in map {
                        let text = v.to_text().to_lowercase();
                        if text.contains(&q) {
                            return true;
                        }
                    }
                }
                false
            })
            .map(|(idx, _)| idx)
            .collect();

        if let Some((sort_col, dir)) = sort {
            let key = ustr(sort_col);
            indices.sort_by(|&a_idx, &b_idx| {
                let v_a = match &items[a_idx] {
                    Val::Map(m) => m.get(&key),
                    _ => None,
                };
                let v_b = match &items[b_idx] {
                    Val::Map(m) => m.get(&key),
                    _ => None,
                };

                let ord = match (v_a, v_b) {
                    (Some(Val::Int(x)), Some(Val::Int(y))) => x.cmp(y),
                    (Some(Val::Float(x)), Some(Val::Float(y))) => {
                        x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal)
                    }
                    (Some(Val::String(x)), Some(Val::String(y))) => x.cmp(y),
                    (Some(x), Some(y)) => x.to_text().cmp(&y.to_text()),
                    (Some(_), None) => std::cmp::Ordering::Greater,
                    (None, Some(_)) => std::cmp::Ordering::Less,
                    (None, None) => std::cmp::Ordering::Equal,
                };

                if *dir == SortDirection::Descending {
                    ord.reverse()
                } else {
                    ord
                }
            });
        }

        indices
    }

    /// Filter and sort rows according to state.
    pub fn prepare_rows(&mut self, _columns: &[String]) -> Vec<&'a Val> {
        let indices = self.state.ensure_indices(self.items).to_vec();
        indices.into_iter().map(|idx| &self.items[idx]).collect()
    }

    pub fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 10 || area.height < 3 {
            return;
        }

        let columns = Self::extract_columns(self.items);
        if columns.is_empty() {
            return;
        }

        let indices = self.state.ensure_indices(self.items).to_vec();
        let total_rows = indices.len();

        // Check if selected row is valid
        if let Some(sel) = self.state.table_state.selected() {
            if total_rows == 0 {
                self.state.table_state.select(None);
            } else if sel >= total_rows {
                self.state.table_state.select(Some(total_rows - 1));
            }
        }

        // Determine column widths and alignments (sample first 100 rows)
        let (widths, is_numeric) = if let (Some(w), Some(num)) =
            (&self.state.cached_widths, &self.state.cached_is_numeric)
        {
            (w.clone(), num.clone())
        } else {
            let mut widths: Vec<usize> = columns.iter().map(|c| c.width().max(6)).collect();
            let mut is_numeric = vec![true; columns.len()];

            for &idx in indices.iter().take(100) {
                if let Some(Val::Map(map)) = self.items.get(idx) {
                    for (i, col) in columns.iter().enumerate() {
                        if let Some(v) = map.get(&ustr(col)) {
                            let text = v.to_text();
                            widths[i] = widths[i].max(text.width().min(60));
                            if !matches!(v, Val::Int(_) | Val::Float(_)) {
                                is_numeric[i] = false;
                            }
                        } else {
                            is_numeric[i] = false;
                        }
                    }
                }
            }
            self.state.cached_widths = Some(widths.clone());
            self.state.cached_is_numeric = Some(is_numeric.clone());
            (widths, is_numeric)
        };

        let rows: Vec<&Val> = indices.iter().map(|&idx| &self.items[idx]).collect();

        // Header cells
        let header_cells = columns.iter().enumerate().map(|(idx, col)| {
            let sort_indicator = match &self.state.sort_column {
                Some((c, dir)) if c == col => format!(" {}", dir.symbol()),
                _ => String::new(),
            };
            let title = format!("{col}{sort_indicator}");
            let base_style = theme::title_style(self.theme);
            let style = if idx == self.state.active_column_idx {
                base_style.add_modifier(
                    ratatui::style::Modifier::UNDERLINED | ratatui::style::Modifier::BOLD,
                )
            } else {
                base_style
            };
            Cell::from(title).style(style)
        });

        let header = Row::new(header_cells)
            .style(theme::title_style(self.theme))
            .bottom_margin(1);

        // Data rows
        let row_items: Vec<Row> = rows
            .iter()
            .enumerate()
            .map(|(r_idx, item)| {
                let is_sel = self.state.table_state.selected() == Some(r_idx);
                let cells: Vec<Cell> = columns
                    .iter()
                    .enumerate()
                    .map(|(c_idx, col)| {
                        let val_str = match item {
                            Val::Map(map) => map
                                .get(&ustr(col))
                                .map(|v| v.to_text())
                                .unwrap_or_else(|| "—".to_string()),
                            _ => String::new(),
                        };

                        let content = if is_numeric[c_idx] {
                            let pad = widths[c_idx].saturating_sub(val_str.width());
                            format!("{}{}", " ".repeat(pad), val_str)
                        } else {
                            val_str
                        };

                        Cell::from(content)
                    })
                    .collect();

                let style = if is_sel {
                    theme::selected_style(self.theme)
                } else {
                    Style::default()
                };

                Row::new(cells).style(style)
            })
            .collect();

        // Constraints
        let constraints: Vec<Constraint> = widths
            .iter()
            .map(|w| Constraint::Length((*w as u16) + 2))
            .collect();

        let table = Table::new(row_items, constraints)
            .header(header)
            .row_highlight_style(theme::selected_style(self.theme))
            .highlight_symbol("❯ ");

        ratatui::widgets::StatefulWidget::render(table, area, buf, &mut self.state.table_state);
    }
}
