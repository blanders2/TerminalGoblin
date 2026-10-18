//! Tabs, tab groups and the panes they own.

use std::collections::HashMap;

use crate::layout::{Dir, Node};
use crate::pane::{Pane, PaneId};
use crate::theme::Color;

pub type TabId = u64;
pub type GroupId = u64;

pub struct Tab {
    pub id: TabId,
    pub root: Node,
    pub focused: PaneId,
    pub custom_title: Option<String>,
    pub color: Option<Color>,
    pub group: Option<GroupId>,
    pub zoomed: bool,
    pub bell: bool,
}

pub struct Group {
    pub id: GroupId,
    pub name: String,
    pub color: Color,
    pub collapsed: bool,
}

#[derive(Default)]
pub struct Workspace {
    pub tabs: Vec<Tab>,
    pub groups: Vec<Group>,
    pub panes: HashMap<PaneId, Pane>,
    pub active: usize,
    next_id: u64,
}

impl Workspace {
    pub fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    pub fn tab_index(&self, id: TabId) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    pub fn group(&self, id: GroupId) -> Option<&Group> {
        self.groups.iter().find(|g| g.id == id)
    }

    pub fn group_mut(&mut self, id: GroupId) -> Option<&mut Group> {
        self.groups.iter_mut().find(|g| g.id == id)
    }

    pub fn focused_pane(&self) -> Option<&Pane> {
        self.active_tab().and_then(|t| self.panes.get(&t.focused))
    }

    pub fn tab_of_pane(&self, pane: PaneId) -> Option<usize> {
        self.tabs.iter().position(|t| t.root.contains(pane))
    }

    pub fn tab_title(&self, tab: &Tab) -> String {
        if let Some(t) = &tab.custom_title {
            return t.clone();
        }
        self.panes
            .get(&tab.focused)
            .map(|p| p.display_title().to_string())
            .unwrap_or_default()
    }

    pub fn is_hidden(&self, tab: &Tab) -> bool {
        tab.group.and_then(|g| self.group(g)).is_some_and(|g| g.collapsed)
    }

    /// Insert a tab after the active one. Like a browser, it joins the active tab's group.
    pub fn insert_tab(&mut self, mut tab: Tab) -> usize {
        let at = if self.tabs.is_empty() { 0 } else { self.active + 1 };
        if tab.group.is_none() {
            tab.group = self.tabs.get(self.active).and_then(|t| t.group);
        }
        self.tabs.insert(at, tab);
        self.activate(at);
        at
    }

    pub fn activate(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        self.active = index;
        let tab = &mut self.tabs[index];
        tab.bell = false;
        if let Some(gid) = tab.group
            && let Some(g) = self.group_mut(gid)
        {
            g.collapsed = false;
        }
    }

    /// Remove a tab and its panes. Returns the panes so the caller can drop them.
    pub fn remove_tab(&mut self, index: usize) -> Vec<Pane> {
        if index >= self.tabs.len() {
            return Vec::new();
        }
        let tab = self.tabs.remove(index);
        let panes = tab
            .root
            .panes()
            .into_iter()
            .filter_map(|id| self.panes.remove(&id))
            .collect();
        self.prune_groups();
        if self.tabs.is_empty() {
            self.active = 0;
            return panes;
        }
        if index < self.active || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        // Prefer a visible neighbour when the closed tab was the active one.
        if self.is_hidden(&self.tabs[self.active])
            && let Some(i) = self.nearest_visible(self.active)
        {
            self.active = i;
        }
        let a = self.active;
        self.activate(a);
        panes
    }

    fn nearest_visible(&self, from: usize) -> Option<usize> {
        (0..self.tabs.len())
            .filter(|&i| !self.is_hidden(&self.tabs[i]))
            .min_by_key(|&i| (i as isize - from as isize).unsigned_abs())
    }

    pub fn cycle(&mut self, forward: bool) {
        let n = self.tabs.len();
        if n == 0 {
            return;
        }
        // Skip tabs in collapsed groups, like a browser would.
        let mut i = self.active;
        for _ in 0..n {
            i = if forward { (i + 1) % n } else { (i + n - 1) % n };
            if !self.is_hidden(&self.tabs[i]) {
                self.activate(i);
                return;
            }
        }
    }

    pub fn move_tab(&mut self, from: usize, to: usize) {
        if from >= self.tabs.len() || to >= self.tabs.len() || from == to {
            return;
        }
        let active_id = self.tabs[self.active].id;
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.fix_group_after_move(to);
        self.active = self.tab_index(active_id).unwrap_or(0);
    }

    /// After a move, a tab joins a group it landed inside of and leaves one it left behind.
    fn fix_group_after_move(&mut self, at: usize) {
        let left = at.checked_sub(1).and_then(|i| self.tabs.get(i)).and_then(|t| t.group);
        let right = self.tabs.get(at + 1).and_then(|t| t.group);
        let own = self.tabs[at].group;
        if left.is_some() && left == right {
            self.tabs[at].group = left;
        } else if own.is_some() && own != left && own != right {
            let still_has_members = self.tabs.iter().enumerate().any(|(i, t)| i != at && t.group == own);
            if still_has_members {
                self.tabs[at].group = None;
            }
        }
        self.prune_groups();
    }

    /// Put a tab into a group, moving it next to the group's other tabs.
    pub fn add_to_group(&mut self, index: usize, gid: GroupId) {
        if index >= self.tabs.len() {
            return;
        }
        let active_id = self.tabs[self.active].id;
        let mut tab = self.tabs.remove(index);
        tab.group = Some(gid);
        let at = self
            .tabs
            .iter()
            .rposition(|t| t.group == Some(gid))
            .map(|i| i + 1)
            .unwrap_or(index.min(self.tabs.len()));
        self.tabs.insert(at, tab);
        self.prune_groups();
        self.active = self.tab_index(active_id).unwrap_or(0);
    }

    /// Move every pane of tab `src` next to `target` (a pane in another tab). The panes keep the
    /// source tab's name and color. Returns the index of the tab they joined.
    pub fn join_tab(&mut self, src: TabId, target: PaneId, dir: Dir) -> Option<usize> {
        let si = self.tab_index(src)?;
        let ti = self.tab_of_pane(target)?;
        if si == ti {
            return None;
        }
        let tab = self.tabs.remove(si);
        for id in tab.root.panes() {
            if let Some(p) = self.panes.get_mut(&id) {
                if tab.custom_title.is_some() {
                    p.label = tab.custom_title.clone();
                }
                if tab.color.is_some() {
                    p.color = tab.color;
                }
            }
        }
        let ti = self.tab_of_pane(target)?;
        let focused = tab.focused;
        let mut node = Some(tab.root);
        let dest = &mut self.tabs[ti];
        dest.root.insert(target, &mut node, dir);
        dest.zoomed = false;
        dest.focused = focused;
        self.prune_groups();
        self.activate(ti);
        Some(ti)
    }

    /// Move a pane out of its tab into a new tab right after it. The pane's name and color
    /// become the new tab's.
    pub fn break_out(&mut self, pane: PaneId) -> Option<usize> {
        let ti = self.tab_of_pane(pane)?;
        if matches!(self.tabs[ti].root, Node::Leaf(_)) {
            return None;
        }
        let tab = &mut self.tabs[ti];
        tab.root.remove(pane);
        tab.zoomed = false;
        if tab.focused == pane {
            tab.focused = tab.root.panes()[0];
        }
        let (title, color) = match self.panes.get_mut(&pane) {
            Some(p) => (p.label.take(), p.color.take()),
            None => (None, None),
        };
        let id = self.next_id();
        self.active = ti;
        let at = self.insert_tab(Tab {
            id,
            root: Node::Leaf(pane),
            focused: pane,
            custom_title: title,
            color,
            group: None,
            zoomed: false,
            bell: false,
        });
        Some(at)
    }

    pub fn new_group(&mut self, index: usize, name: String, color: Color) -> GroupId {
        let id = self.next_id();
        self.groups.push(Group {
            id,
            name,
            color,
            collapsed: false,
        });
        self.add_to_group(index, id);
        id
    }

    pub fn ungroup_tab(&mut self, index: usize) {
        let Some(gid) = self.tabs.get(index).and_then(|t| t.group) else {
            return;
        };
        let active_id = self.tabs[self.active].id;
        // Move the tab just past the end of its group so the group stays contiguous.
        let mut tab = self.tabs.remove(index);
        tab.group = None;
        let at = self
            .tabs
            .iter()
            .rposition(|t| t.group == Some(gid))
            .map(|i| i + 1)
            .unwrap_or(index.min(self.tabs.len()));
        self.tabs.insert(at, tab);
        self.prune_groups();
        self.active = self.tab_index(active_id).unwrap_or(0);
    }

    pub fn dissolve_group(&mut self, gid: GroupId) {
        for t in &mut self.tabs {
            if t.group == Some(gid) {
                t.group = None;
            }
        }
        self.prune_groups();
    }

    /// Collapse or expand a group. The active tab moves out of a group being collapsed.
    pub fn toggle_collapse(&mut self, gid: GroupId) {
        let Some(collapsed) = self.group(gid).map(|g| g.collapsed) else {
            return;
        };
        if collapsed {
            if let Some(g) = self.group_mut(gid) {
                g.collapsed = false;
            }
            return;
        }
        let outside = (0..self.tabs.len())
            .filter(|&i| self.tabs[i].group != Some(gid) && !self.is_hidden(&self.tabs[i]))
            .min_by_key(|&i| (i as isize - self.active as isize).unsigned_abs());
        let active_inside = self.tabs.get(self.active).is_some_and(|t| t.group == Some(gid));
        if active_inside {
            match outside {
                Some(i) => self.active = i,
                None => return, // Nothing else to show; refuse to hide every tab.
            }
        }
        if let Some(g) = self.group_mut(gid) {
            g.collapsed = true;
        }
    }

    fn prune_groups(&mut self) {
        let tabs = &self.tabs;
        self.groups.retain(|g| tabs.iter().any(|t| t.group == Some(g.id)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(ws: &mut Workspace) -> TabId {
        let id = ws.next_id();
        ws.insert_tab(Tab {
            id,
            root: Node::Leaf(id * 100),
            focused: id * 100,
            custom_title: None,
            color: None,
            group: None,
            zoomed: false,
            bell: false,
        });
        id
    }

    fn order(ws: &Workspace) -> Vec<TabId> {
        ws.tabs.iter().map(|t| t.id).collect()
    }

    #[test]
    fn grouping_keeps_members_contiguous() {
        let mut ws = Workspace::default();
        let a = tab(&mut ws);
        let b = tab(&mut ws);
        let c = tab(&mut ws);
        let g = ws.new_group(0, "g".into(), [1.0; 4]);
        ws.add_to_group(ws.tab_index(c).unwrap(), g);
        assert_eq!(order(&ws), vec![a, c, b]);
        assert_eq!(ws.tabs[1].group, Some(g));
        ws.ungroup_tab(0);
        assert_eq!(order(&ws), vec![c, a, b], "ungrouped tab moves past the group");
        assert_eq!(ws.tabs[0].group, Some(g));
        ws.ungroup_tab(0);
        assert!(ws.groups.is_empty(), "empty groups are pruned");
    }

    #[test]
    fn new_tab_joins_active_group() {
        let mut ws = Workspace::default();
        tab(&mut ws);
        let g = ws.new_group(0, "g".into(), [1.0; 4]);
        let b = tab(&mut ws);
        assert_eq!(ws.tabs[ws.tab_index(b).unwrap()].group, Some(g));
    }

    #[test]
    fn collapsing_moves_active_out_of_group() {
        let mut ws = Workspace::default();
        let a = tab(&mut ws);
        let b = tab(&mut ws);
        let g = ws.new_group(ws.tab_index(b).unwrap(), "g".into(), [1.0; 4]);
        ws.activate(ws.tab_index(b).unwrap());
        ws.toggle_collapse(g);
        assert!(ws.group(g).unwrap().collapsed);
        assert_eq!(ws.tabs[ws.active].id, a);
        ws.cycle(true);
        assert_eq!(ws.tabs[ws.active].id, a, "cycling skips collapsed tabs");
        ws.activate(ws.tab_index(b).unwrap());
        assert!(
            !ws.group(g).unwrap().collapsed,
            "activating a hidden tab expands its group"
        );
    }

    #[test]
    fn refuses_to_collapse_only_visible_tabs() {
        let mut ws = Workspace::default();
        tab(&mut ws);
        let g = ws.new_group(0, "g".into(), [1.0; 4]);
        ws.toggle_collapse(g);
        assert!(!ws.group(g).unwrap().collapsed);
    }

    #[test]
    fn removing_tabs_keeps_active_valid() {
        let mut ws = Workspace::default();
        let a = tab(&mut ws);
        let _b = tab(&mut ws);
        let c = tab(&mut ws);
        assert_eq!(ws.tabs[ws.active].id, c);
        ws.remove_tab(2);
        assert_eq!(ws.active, 1);
        ws.remove_tab(1);
        assert_eq!(ws.tabs[ws.active].id, a);
        ws.remove_tab(0);
        assert!(ws.tabs.is_empty());
    }

    #[test]
    fn join_and_break_out() {
        let mut ws = Workspace::default();
        let a = tab(&mut ws);
        let b = tab(&mut ws);
        let c = tab(&mut ws);
        // Join c (pane 300) to the right of a's pane (100).
        let at = ws.join_tab(c, a * 100, Dir::Right).unwrap();
        assert_eq!(order(&ws), vec![a, b]);
        assert_eq!(at, 0);
        assert_eq!(ws.active, 0);
        assert_eq!(ws.tabs[0].root.panes(), vec![100, 300]);
        assert_eq!(ws.tabs[0].focused, 300, "the joined pane gets focus");
        assert!(ws.join_tab(a, 300, Dir::Left).is_none(), "can't join a tab into itself");

        let at = ws.break_out(300).unwrap();
        assert_eq!(at, 1);
        assert_eq!(ws.tabs.len(), 3);
        assert_eq!(ws.tabs[1].root.panes(), vec![300]);
        assert_eq!(ws.tabs[0].root.panes(), vec![100]);
        assert_eq!(ws.tabs[0].focused, 100);
        assert!(ws.break_out(300).is_none(), "a lone pane is already its own tab");
    }

    #[test]
    fn moving_a_tab_into_a_group_joins_it() {
        let mut ws = Workspace::default();
        let a = tab(&mut ws);
        let b = tab(&mut ws);
        let c = tab(&mut ws);
        let g = ws.new_group(0, "g".into(), [1.0; 4]);
        ws.add_to_group(ws.tab_index(b).unwrap(), g);
        // a, b grouped; drag c between them.
        ws.move_tab(ws.tab_index(c).unwrap(), 1);
        assert_eq!(order(&ws), vec![a, c, b]);
        assert_eq!(ws.tabs[1].group, Some(g));
    }
}
