//! Browser state that is neither a tab nor a widget: which tab to reach next, and which
//! ones can be brought back.
//!
//! These used to be bare `RefCell` fields on the GTK window, which meant the rules lived
//! in signal handlers and could not be tested without a display. Both types below have
//! invariants that have been got wrong before; the tests at the bottom pin them.

use crate::tab::TabId;

/// How many closed tabs are remembered for Ctrl+Shift+T.
const CLOSED_TAB_LIMIT: usize = 25;

/// Tabs in most-recently-used order, most recent first, plus the state of an in-flight
/// Ctrl+Tab cycle.
///
/// The list is deliberately *not* reordered while cycling: repeated presses have to walk
/// to the third tab and beyond rather than bouncing between the first two. The cycle ends
/// when the caller's settle timer fires and calls [`MruList::commit`].
#[derive(Debug, Default, Clone)]
pub struct MruList {
    order: Vec<TabId>,
    /// Position within `order` while a cycle is in flight; `None` when not cycling.
    cycle: Option<usize>,
}

impl MruList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub fn contains(&self, tab_id: TabId) -> bool {
        self.order.contains(&tab_id)
    }

    pub fn is_cycling(&self) -> bool {
        self.cycle.is_some()
    }

    /// Add a freshly opened tab at the *back*: it is reachable by cycling, but it has not
    /// been used yet. Without this only activated tabs ever entered the list, so opening
    /// several tabs and never switching left one entry and Ctrl+Tab did nothing at all.
    pub fn insert_unused(&mut self, tab_id: TabId) {
        if !self.contains(tab_id) {
            self.order.push(tab_id);
        }
    }

    /// Move `tab_id` to the front (inserting it if new). Ignored while cycling, since
    /// activating tabs is exactly what cycling does.
    pub fn touch(&mut self, tab_id: TabId) {
        if self.is_cycling() {
            return;
        }
        self.order.retain(|id| *id != tab_id);
        self.order.insert(0, tab_id);
    }

    /// Drop a closed tab. Returns `true` when the caller must also abandon its settle
    /// timer, because there is no longer enough left to cycle through.
    pub fn forget(&mut self, tab_id: TabId) -> bool {
        self.order.retain(|id| *id != tab_id);
        if self.order.len() < 2 {
            self.cancel();
            return true;
        }
        false
    }

    /// Ctrl+Tab (`+1`) / Ctrl+Shift+Tab (`-1`): the tab to activate next, or `None` when
    /// there is nothing to cycle between.
    pub fn step(&mut self, direction: i32) -> Option<TabId> {
        if self.order.len() < 2 {
            return None;
        }
        let from = self.cycle.unwrap_or(0) as i32;
        let next = (from + direction).rem_euclid(self.order.len() as i32) as usize;
        self.cycle = Some(next);
        Some(self.order[next])
    }

    /// End the cycle, promoting whichever tab it landed on. A no-op when no cycle was in
    /// flight, so a stray settle timer cannot reorder the list.
    pub fn commit(&mut self, active: Option<TabId>) {
        if self.cycle.take().is_none() {
            return;
        }
        if let Some(tab_id) = active {
            self.touch(tab_id);
        }
    }

    /// Abandon a cycle without promoting anything.
    pub fn cancel(&mut self) {
        self.cycle = None;
    }
}

/// Recently closed tabs, newest last; Ctrl+Shift+T pops the stack.
#[derive(Debug, Default, Clone)]
pub struct ClosedTabs {
    stack: Vec<ClosedTab>,
}

/// A tab that was closed, and where it sat in the strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosedTab {
    pub url: String,
    /// Strip position to restore it to; `None` reopens at the end.
    pub position: Option<usize>,
}

impl ClosedTabs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    /// Remember a closed tab, discarding the oldest once the limit is reached.
    pub fn push(&mut self, url: String, position: Option<usize>) {
        self.stack.push(ClosedTab { url, position });
        let overflow = self.stack.len().saturating_sub(CLOSED_TAB_LIMIT);
        self.stack.drain(..overflow);
    }

    /// The most recently closed tab.
    pub fn pop(&mut self) -> Option<ClosedTab> {
        self.stack.pop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<TabId> {
        (0..n).map(|_| TabId::new()).collect()
    }

    #[test]
    fn touch_moves_to_the_front_without_duplicating() {
        let t = ids(3);
        let mut mru = MruList::new();
        for id in &t {
            mru.touch(*id);
        }
        mru.touch(t[2]);
        assert_eq!(mru.len(), 3);
        assert_eq!(mru.step(0), Some(t[2]));
    }

    #[test]
    fn a_new_tab_enters_at_the_back_so_it_is_cycleable() {
        let t = ids(2);
        let mut mru = MruList::new();
        mru.touch(t[0]);
        mru.insert_unused(t[1]);
        // Two entries is enough to cycle; before this existed the list held one and
        // Ctrl+Tab silently did nothing.
        assert_eq!(mru.step(1), Some(t[1]));
    }

    #[test]
    fn insert_unused_does_not_demote_a_tab_already_listed() {
        let t = ids(2);
        let mut mru = MruList::new();
        mru.touch(t[0]);
        mru.touch(t[1]);
        mru.insert_unused(t[1]);
        assert_eq!(mru.len(), 2);
        assert_eq!(mru.step(0), Some(t[1]));
    }

    #[test]
    fn cycling_walks_past_the_second_tab() {
        let t = ids(3);
        let mut mru = MruList::new();
        for id in t.iter().rev() {
            mru.touch(*id);
        }
        // order is now [t0, t1, t2]
        assert_eq!(mru.step(1), Some(t[1]));
        assert_eq!(mru.step(1), Some(t[2]));
        // ...and wraps rather than stopping.
        assert_eq!(mru.step(1), Some(t[0]));
    }

    #[test]
    fn activating_during_a_cycle_does_not_reorder() {
        let t = ids(3);
        let mut mru = MruList::new();
        for id in t.iter().rev() {
            mru.touch(*id);
        }
        mru.step(1);
        // The frontend activates the tab it stepped to; that must not promote it, or the
        // next press would bounce back instead of moving on.
        mru.touch(t[1]);
        assert_eq!(mru.step(1), Some(t[2]));
    }

    #[test]
    fn stepping_backwards_wraps_to_the_end() {
        let t = ids(3);
        let mut mru = MruList::new();
        for id in t.iter().rev() {
            mru.touch(*id);
        }
        assert_eq!(mru.step(-1), Some(t[2]));
    }

    #[test]
    fn commit_promotes_the_landed_tab_and_ends_the_cycle() {
        let t = ids(3);
        let mut mru = MruList::new();
        for id in t.iter().rev() {
            mru.touch(*id);
        }
        mru.step(1);
        assert!(mru.is_cycling());
        mru.commit(Some(t[1]));
        assert!(!mru.is_cycling());
        assert_eq!(mru.step(0), Some(t[1]));
    }

    #[test]
    fn commit_without_a_cycle_in_flight_changes_nothing() {
        let t = ids(2);
        let mut mru = MruList::new();
        mru.touch(t[1]);
        mru.touch(t[0]);
        mru.commit(Some(t[1]));
        assert_eq!(mru.step(0), Some(t[0]));
    }

    #[test]
    fn fewer_than_two_tabs_cannot_cycle() {
        let mut mru = MruList::new();
        assert_eq!(mru.step(1), None);
        mru.touch(TabId::new());
        assert_eq!(mru.step(1), None);
    }

    #[test]
    fn forgetting_down_to_one_tab_asks_the_caller_to_cancel() {
        let t = ids(3);
        let mut mru = MruList::new();
        for id in &t {
            mru.touch(*id);
        }
        mru.step(1);
        assert!(!mru.forget(t[0]));
        assert!(mru.forget(t[1]));
        assert!(!mru.is_cycling());
    }

    #[test]
    fn closed_tabs_pop_newest_first_and_keep_their_position() {
        let mut closed = ClosedTabs::new();
        assert!(closed.is_empty());
        closed.push("https://a.example/".into(), Some(2));
        closed.push("https://b.example/".into(), None);
        assert_eq!(
            closed.pop(),
            Some(ClosedTab {
                url: "https://b.example/".into(),
                position: None
            })
        );
        assert_eq!(
            closed.pop(),
            Some(ClosedTab {
                url: "https://a.example/".into(),
                position: Some(2)
            })
        );
        assert!(closed.pop().is_none());
    }

    #[test]
    fn closed_tabs_discard_the_oldest_past_the_limit() {
        let mut closed = ClosedTabs::new();
        for i in 0..CLOSED_TAB_LIMIT + 5 {
            closed.push(format!("https://example.com/{i}"), None);
        }
        let mut seen = 0;
        while let Some(tab) = closed.pop() {
            seen += 1;
            // The five oldest are gone, so nothing below /5 survives.
            let n: usize = tab.url.rsplit('/').next().unwrap().parse().unwrap();
            assert!(n >= 5, "kept an entry that should have been dropped: {n}");
        }
        assert_eq!(seen, CLOSED_TAB_LIMIT);
    }
}
