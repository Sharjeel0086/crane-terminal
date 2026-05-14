//! Row of cells with a dirty upper bound.
//!
//! `occ` is the largest column index the row has ever been touched
//! at, plus one. Cells from `occ..len` are guaranteed to equal the
//! template that the row was reset against, so dirty iteration and
//! redraw can stop at `occ` and skip the empty tail.

use crate::cell::Cell;

#[derive(Clone, Debug)]
pub struct Row {
    pub cells: Vec<Cell>,
    /// Upper bound on touched columns. `cells[occ..]` is template-
    /// equal. Reset by [`Row::reset`].
    pub occ: usize,
    /// Last [`Term::current_sync_gen`] value this row was mutated
    /// under. Compared against the live gen in `scroll_up_one` to
    /// decide whether an evicted row is intermediate redraw state
    /// (drop) or genuine pre-sync history (keep in scrollback).
    /// Always 0 in tests / code paths that pre-date the gate; the
    /// inequality test still works because the live gen starts at 1.
    pub written_in_gen: u64,
}

impl Row {
    pub fn new(columns: usize, template: &Cell) -> Self {
        Self {
            cells: vec![template.clone(); columns],
            occ: 0,
            written_in_gen: 0,
        }
    }

    /// Reset every column to `template` and clear the dirty bound.
    /// Used when scrolling pulls a row back into view.
    pub fn reset(&mut self, template: &Cell) {
        for c in self.cells.iter_mut().take(self.occ) {
            *c = template.clone();
        }
        self.occ = 0;
    }

    /// Mark the row as dirty up to (and including) column `col`.
    /// Should be called by every code path that mutates a cell in
    /// place — keeps `occ` honest.
    pub fn mark_touched(&mut self, col: usize) {
        let bound = col.saturating_add(1).min(self.cells.len());
        if bound > self.occ {
            self.occ = bound;
        }
    }

    /// Convenience: [`mark_touched`] plus tag with the current
    /// sync gen. Use at every cell-write site in [`Term`] so
    /// `scroll_up_one` can distinguish pre-sync history rows from
    /// rows mutated by the current `?2026` block.
    pub fn touched_at(&mut self, col: usize, sync_gen: u64) {
        self.mark_touched(col);
        self.written_in_gen = sync_gen;
    }

    /// Convenience: [`reset`] plus tag with the current sync gen.
    pub fn reset_at(&mut self, template: &Cell, sync_gen: u64) {
        self.reset(template);
        self.written_in_gen = sync_gen;
    }

    /// Resize the row to `cols` columns, padding with `template` on
    /// growth. Used when the terminal viewport resizes.
    pub fn resize(&mut self, cols: usize, template: &Cell) {
        if cols > self.cells.len() {
            self.cells.resize(cols, template.clone());
        } else if cols < self.cells.len() {
            self.cells.truncate(cols);
            if self.occ > cols {
                self.occ = cols;
            }
        }
    }
}
