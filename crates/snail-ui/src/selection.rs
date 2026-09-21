//! List selection (plan.md E5.8), framework-free and unit-tested.
//!
//! Selection is tracked **by id, not index**, so a sync that inserts or removes rows above it does
//! not move the cursor to a different message. `reconcile` is the only thing a sync has to call.

use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    anchor: Option<i64>,
    cursor: Option<i64>,
    extra: BTreeSet<i64>,
    /// Where the cursor last sat, so a removal can move to the row that takes its place.
    last_index: usize,
}

impl Selection {
    pub fn new() -> Self {
        Self::default()
    }

    /// Select exactly one id.
    pub fn select(&mut self, id: i64) {
        self.anchor = Some(id);
        self.cursor = Some(id);
        self.extra.clear();
    }

    /// Select one id, remembering where it sits so a later removal can move to its neighbour.
    pub fn select_in(&mut self, ids: &[i64], id: i64) {
        self.select(id);
        if let Some(index) = ids.iter().position(|candidate| *candidate == id) {
            self.last_index = index;
        }
    }

    pub fn cursor(&self) -> Option<i64> {
        self.cursor
    }

    pub fn anchor(&self) -> Option<i64> {
        self.anchor
    }

    pub fn is_selected(&self, id: i64) -> bool {
        self.cursor == Some(id) || self.extra.contains(&id)
    }

    pub fn selected_ids(&self) -> Vec<i64> {
        let mut ids = self.extra.clone();
        if let Some(cursor) = self.cursor {
            ids.insert(cursor);
        }
        ids.into_iter().collect()
    }

    /// Move the cursor by `delta` rows in `ids`, collapsing the selection.
    pub fn move_by(&mut self, delta: isize, ids: &[i64]) {
        if ids.is_empty() {
            return;
        }
        let current = self
            .cursor
            .and_then(|id| ids.iter().position(|candidate| *candidate == id))
            .unwrap_or(0);
        let next = (current as isize + delta).clamp(0, ids.len() as isize - 1) as usize;
        self.select(ids[next]);
        self.last_index = next;
    }

    /// Move the cursor, extending the selection from the anchor (Shift+↑/↓).
    pub fn extend_by(&mut self, delta: isize, ids: &[i64]) {
        if ids.is_empty() {
            return;
        }
        let current = self
            .cursor
            .and_then(|id| ids.iter().position(|candidate| *candidate == id))
            .unwrap_or(0);
        let next = (current as isize + delta).clamp(0, ids.len() as isize - 1) as usize;
        self.cursor = Some(ids[next]);
        self.last_index = next;
        self.rebuild_range(ids);
    }

    /// Toggle one id into or out of the selection (⌘/Ctrl-click).
    pub fn toggle(&mut self, id: i64) {
        if self.cursor == Some(id) {
            return;
        }
        if !self.extra.remove(&id) {
            self.extra.insert(id);
        }
        self.anchor = Some(id);
    }

    /// Reconcile with the current order after a sync. The cursor follows its message when rows are
    /// inserted above it; when the cursor's message is gone, the item that now occupies its old
    /// position (the next row) is selected — never the top.
    pub fn reconcile(&mut self, ids: &[i64]) {
        let present = |id: i64| ids.contains(&id);
        self.extra.retain(|id| present(*id));

        match self.cursor {
            Some(cursor) if present(cursor) => {
                self.last_index = ids.iter().position(|id| *id == cursor).unwrap_or(0);
            }
            Some(_) | None if ids.is_empty() => {
                self.cursor = None;
                self.anchor = None;
            }
            Some(_) => {
                let index = self.last_index.min(ids.len() - 1);
                self.select(ids[index]);
            }
            None => {}
        }

        if let Some(cursor) = self.cursor {
            if !present(cursor) {
                self.cursor = None;
            }
        }
    }

    fn rebuild_range(&mut self, ids: &[i64]) {
        self.extra.clear();
        let (Some(anchor), Some(cursor)) = (self.anchor, self.cursor) else {
            return;
        };
        let Some(a) = ids.iter().position(|id| *id == anchor) else {
            return;
        };
        let Some(b) = ids.iter().position(|id| *id == cursor) else {
            return;
        };
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        for id in &ids[start..=end] {
            self.extra.insert(*id);
        }
        self.extra.remove(&cursor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> Vec<i64> {
        vec![10, 20, 30, 40, 50]
    }

    #[test]
    fn moves_clamp_to_the_list() {
        let mut selection = Selection::new();
        selection.select(10);
        selection.move_by(-1, &ids());
        assert_eq!(selection.cursor(), Some(10));
        selection.move_by(10, &ids());
        assert_eq!(selection.cursor(), Some(50));
    }

    #[test]
    fn shift_extends_from_the_anchor() {
        let mut selection = Selection::new();
        selection.select(20);
        selection.extend_by(2, &ids());
        assert_eq!(selection.cursor(), Some(40));
        assert!(selection.is_selected(20) && selection.is_selected(30) && selection.is_selected(40));
        assert!(!selection.is_selected(10));
    }

    #[test]
    fn cmd_toggle_adds_and_removes() {
        let mut selection = Selection::new();
        selection.select(10);
        selection.toggle(30);
        assert!(selection.is_selected(30));
        selection.toggle(30);
        assert!(!selection.is_selected(30));
    }

    #[test]
    fn selection_follows_its_message_when_rows_are_inserted_above() {
        let mut selection = Selection::new();
        selection.select_in(&ids(), 30);
        // A sync delivered three newer rows above.
        selection.reconcile(&[1, 2, 3, 10, 20, 30, 40, 50]);
        assert_eq!(selection.cursor(), Some(30), "the cursor must not jump to a new message");
    }

    #[test]
    fn deleting_the_selection_moves_to_the_next_row_not_the_top() {
        let mut selection = Selection::new();
        selection.select_in(&ids(), 30);
        // 30 is gone; 40 now occupies its position.
        selection.reconcile(&[10, 20, 40, 50]);
        assert_eq!(selection.cursor(), Some(40));
    }

    #[test]
    fn deleting_the_last_row_falls_back_to_the_new_last() {
        let mut selection = Selection::new();
        selection.select_in(&ids(), 50);
        selection.reconcile(&[10, 20, 30, 40]);
        assert_eq!(selection.cursor(), Some(40));
    }

    #[test]
    fn an_empty_list_clears_the_selection() {
        let mut selection = Selection::new();
        selection.select(30);
        selection.reconcile(&[]);
        assert_eq!(selection.cursor(), None);
    }
}
