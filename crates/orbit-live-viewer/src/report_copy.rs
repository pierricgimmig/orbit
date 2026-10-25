// Copyright (c) 2026 The Orbit Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Turning a report table into clipboard text.
//!
//! The report tabs paint their rows straight to the canvas, so nothing is
//! selectable the way a web table is. Instead each renderer records the rows
//! it laid out -- in the order and with the columns the user sees -- as a
//! [`ReportView`], and a drag selects rows by id. Ctrl+C (or the Copy button)
//! turns the selection, or the whole view when nothing is selected, into an
//! aligned monospace table that pastes cleanly into a document or chat.
//!
//! One column is the "name" column and is left-aligned and indented (call
//! trees prepend spaces per depth when they build their cells); numeric
//! columns are right-aligned so the digits line up.

use std::collections::HashSet;

/// One row of a report, as the strings shown, with the id a selection and a
/// hook use (0 when the row has no resolvable function).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReportRow {
    pub id: u64,
    pub cols: Vec<String>,
}

/// A snapshot of the table currently shown, enough to copy it.
#[derive(Clone, Debug, Default)]
pub struct ReportView {
    pub headers: Vec<&'static str>,
    /// Right-align this column (numbers), else left (text). Same length as
    /// `headers`.
    pub right: Vec<bool>,
    pub rows: Vec<ReportRow>,
}

impl ReportView {
    pub fn new(headers: &[&'static str], right: &[bool]) -> ReportView {
        ReportView { headers: headers.to_vec(), right: right.to_vec(), rows: Vec::new() }
    }

    pub fn push(&mut self, id: u64, cols: Vec<String>) {
        self.rows.push(ReportRow { id, cols });
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The number of rows a copy would produce for `selection` (the selected
    /// rows, or all when the selection is empty). Drives the "Copy N" label.
    pub fn copy_count(&self, selection: &HashSet<u64>) -> usize {
        if selection.is_empty() {
            self.rows.len()
        } else {
            self.rows.iter().filter(|r| r.id != 0 && selection.contains(&r.id)).count()
        }
    }

    /// The clipboard text: the selected rows in view order, or every row when
    /// the selection is empty, as an aligned table with a header. Empty string
    /// when there is nothing to copy.
    pub fn to_text(&self, selection: &HashSet<u64>) -> String {
        let take_all = selection.is_empty();
        let chosen: Vec<&ReportRow> = self
            .rows
            .iter()
            .filter(|r| take_all || (r.id != 0 && selection.contains(&r.id)))
            .collect();
        if chosen.is_empty() {
            return String::new();
        }
        let ncols = self.headers.len();
        let mut width = vec![0usize; ncols];
        for (c, h) in self.headers.iter().enumerate() {
            width[c] = h.chars().count();
        }
        for row in &chosen {
            for c in 0..ncols {
                let len = row.cols.get(c).map(|s| s.chars().count()).unwrap_or(0);
                width[c] = width[c].max(len);
            }
        }
        let right = |c: usize| self.right.get(c).copied().unwrap_or(false);
        let mut out = String::new();
        let mut line = |cells: &[String]| {
            let mut row = String::new();
            for c in 0..ncols {
                if c > 0 {
                    row.push_str("  ");
                }
                let cell = cells.get(c).map(String::as_str).unwrap_or("");
                let pad = width[c].saturating_sub(cell.chars().count());
                if right(c) {
                    row.push_str(&" ".repeat(pad));
                    row.push_str(cell);
                } else {
                    row.push_str(cell);
                    row.push_str(&" ".repeat(pad));
                }
            }
            // No line carries trailing spaces (an empty last cell, a padded
            // last column).
            out.push_str(row.trim_end());
            out.push('\n');
        };
        let header: Vec<String> = self.headers.iter().map(|h| h.to_string()).collect();
        line(&header);
        for row in &chosen {
            line(&row.cols);
        }
        out
    }
}

/// A percentage as the reports show it: one decimal, with the sign.
pub fn pct(v: f32) -> String {
    format!("{v:.1}%")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> ReportView {
        let mut v = ReportView::new(&["self%", "incl%", "function", "module"], &[true, true, false, false]);
        v.push(1, vec![pct(40.1), pct(40.1), "SolveContacts".into(), "libbox3d.a".into()]);
        v.push(2, vec![pct(5.0), pct(60.0), "step".into(), "app".into()]);
        v.push(0, vec![pct(1.0), pct(1.0), "0x1234".into(), "".into()]);
        v
    }

    #[test]
    fn empty_selection_copies_every_row_with_a_header() {
        let text = view().to_text(&HashSet::new());
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4, "{text}");
        assert!(lines[0].starts_with("self%"), "{}", lines[0]);
        // Columns align: the function column starts at the same offset on
        // every line.
        let at = |s: &str| s.find("function").or_else(|| s.find("SolveContacts")).unwrap();
        assert_eq!(at(lines[0]), at(lines[1]), "columns must line up:\n{text}");
        // Numbers are right-aligned: "40.1%" and "5.0%" end at the same column.
        assert!(lines[1].contains("40.1%") && lines[2].contains(" 5.0%"), "{text}");
    }

    #[test]
    fn a_selection_copies_only_those_rows() {
        let sel: HashSet<u64> = [1u64].into_iter().collect();
        let text = view().to_text(&sel);
        assert_eq!(text.lines().count(), 2, "header + one row: {text}");
        assert!(text.contains("SolveContacts") && !text.contains("step"), "{text}");
    }

    #[test]
    fn id_zero_rows_are_uncopyable_by_selection_but_come_with_all() {
        // A selection never picks an id-0 row (it shares 0 with every other).
        let sel: HashSet<u64> = [0u64].into_iter().collect();
        assert_eq!(view().to_text(&sel), "", "id 0 cannot be selected");
        // But copy-all includes it.
        assert!(view().to_text(&HashSet::new()).contains("0x1234"));
    }

    #[test]
    fn no_trailing_whitespace_on_any_line() {
        let text = view().to_text(&HashSet::new());
        for line in text.lines() {
            assert_eq!(line, line.trim_end(), "trailing space: {line:?}");
        }
    }

    #[test]
    fn copy_count_tracks_the_selection() {
        let v = view();
        assert_eq!(v.copy_count(&HashSet::new()), 3);
        assert_eq!(v.copy_count(&[1u64, 2].into_iter().collect()), 2);
        assert_eq!(v.copy_count(&[0u64].into_iter().collect()), 0);
    }
}
