use crate::app_event::SplitAxis;
use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::style::Stylize;

const PANE_DIVIDER_THICKNESS: u16 = 1;

pub(crate) const MIN_PANE_WIDTH: u16 = 48;
pub(crate) const MIN_PANE_HEIGHT: u16 = 12;

pub(crate) type PaneId = u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SplitPaneState<T> {
    root: SplitPaneNode<T>,
    active_pane_id: PaneId,
    next_pane_id: PaneId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SplitPaneNode<T> {
    Leaf {
        pane_id: PaneId,
        thread_id: T,
    },
    Split {
        axis: SplitAxis,
        first: Box<SplitPaneNode<T>>,
        second: Box<SplitPaneNode<T>>,
    },
}

impl<T: Copy + Eq> SplitPaneState<T> {
    pub(crate) fn new(thread_id: T) -> Self {
        Self {
            root: SplitPaneNode::Leaf {
                pane_id: 0,
                thread_id,
            },
            active_pane_id: 0,
            next_pane_id: 1,
        }
    }

    pub(crate) fn desired_height() -> u16 {
        u16::MAX
    }

    pub(crate) fn active_pane_id(&self) -> PaneId {
        self.active_pane_id
    }

    pub(crate) fn active_thread(&self) -> T {
        self.root
            .thread_for_pane(self.active_pane_id)
            .expect("active pane should exist")
    }

    pub(crate) fn set_active_thread(&mut self, thread_id: T) {
        self.root
            .set_thread_for_pane(self.active_pane_id, thread_id);
    }

    pub(crate) fn split_active(&mut self, thread_id: T, axis: SplitAxis) -> PaneId {
        let new_pane_id = self.next_pane_id;
        self.next_pane_id += 1;
        self.root
            .split_pane(self.active_pane_id, new_pane_id, thread_id, axis);
        self.active_pane_id = new_pane_id;
        new_pane_id
    }

    pub(crate) fn focus_pane(&mut self, pane_id: PaneId) -> Option<T> {
        let thread_id = self.root.thread_for_pane(pane_id)?;
        self.active_pane_id = pane_id;
        Some(thread_id)
    }

    pub(crate) fn focus_next(&mut self) -> Option<T> {
        self.focus_by_offset(1)
    }

    pub(crate) fn focus_previous(&mut self) -> Option<T> {
        self.focus_by_offset(-1)
    }

    pub(crate) fn leaf_count(&self) -> usize {
        self.root.leaf_count()
    }

    pub(crate) fn contains_thread(&self, thread_id: T) -> bool {
        self.root.contains_thread(thread_id)
    }

    pub(crate) fn pane_for_thread(&self, thread_id: T) -> Option<PaneId> {
        self.root.pane_for_thread(thread_id)
    }

    pub(crate) fn leaves(&self) -> Vec<(PaneId, T)> {
        let mut leaves = Vec::new();
        self.root.collect_leaves(&mut leaves);
        leaves
    }

    pub(crate) fn inactive_leaves(&self) -> Vec<(PaneId, T)> {
        self.leaves()
            .into_iter()
            .filter(|(pane_id, _)| *pane_id != self.active_pane_id)
            .collect()
    }

    pub(crate) fn can_split_active(&self, area: Rect, axis: SplitAxis) -> bool {
        let Some(active_area) = self.active_pane_area(area) else {
            return false;
        };
        let (first, _, second) = split_regions(active_area, axis);
        first.width >= MIN_PANE_WIDTH
            && second.width >= MIN_PANE_WIDTH
            && first.height >= MIN_PANE_HEIGHT
            && second.height >= MIN_PANE_HEIGHT
    }

    pub(crate) fn active_pane_area(&self, area: Rect) -> Option<Rect> {
        self.root.pane_area(self.active_pane_id, area)
    }

    pub(crate) fn without_thread(mut self, thread_id: T) -> Option<Self> {
        if !self.contains_thread(thread_id) {
            return Some(self);
        }
        self.root = self.root.without_thread(thread_id)?;
        if !self.root.contains_pane(self.active_pane_id) {
            self.active_pane_id = self
                .root
                .first_pane_id()
                .expect("non-empty split panes should still contain a pane");
        }
        Some(self)
    }

    pub(crate) fn visit_leaf_areas<F>(&self, area: Rect, visit: &mut F)
    where
        F: FnMut(PaneId, T, Rect),
    {
        self.root.visit_leaf_areas(area, visit);
    }

    pub(crate) fn render_dividers(&self, area: Rect, buf: &mut Buffer) {
        self.root.render_dividers(area, buf, self.active_pane_id);
    }

    fn focus_by_offset(&mut self, offset: isize) -> Option<T> {
        let leaves = self.leaves();
        if leaves.len() <= 1 {
            return None;
        }
        let active_index = leaves
            .iter()
            .position(|(pane_id, _)| *pane_id == self.active_pane_id)?;
        let len = leaves.len() as isize;
        let next_index = (active_index as isize + offset).rem_euclid(len) as usize;
        let (pane_id, thread_id) = leaves[next_index];
        self.active_pane_id = pane_id;
        Some(thread_id)
    }
}

impl<T: Copy + Eq> SplitPaneNode<T> {
    fn split_pane(
        &mut self,
        target_pane_id: PaneId,
        new_pane_id: PaneId,
        new_thread_id: T,
        axis: SplitAxis,
    ) -> bool {
        match self {
            SplitPaneNode::Leaf { pane_id, thread_id } => {
                if *pane_id != target_pane_id {
                    return false;
                }
                let current_pane_id = *pane_id;
                let current_thread_id = *thread_id;
                *self = SplitPaneNode::Split {
                    axis,
                    first: Box::new(SplitPaneNode::Leaf {
                        pane_id: current_pane_id,
                        thread_id: current_thread_id,
                    }),
                    second: Box::new(SplitPaneNode::Leaf {
                        pane_id: new_pane_id,
                        thread_id: new_thread_id,
                    }),
                };
                true
            }
            SplitPaneNode::Split { first, second, .. } => {
                first.split_pane(target_pane_id, new_pane_id, new_thread_id, axis)
                    || second.split_pane(target_pane_id, new_pane_id, new_thread_id, axis)
            }
        }
    }

    fn thread_for_pane(&self, target_pane_id: PaneId) -> Option<T> {
        match self {
            SplitPaneNode::Leaf { pane_id, thread_id } => {
                (*pane_id == target_pane_id).then_some(*thread_id)
            }
            SplitPaneNode::Split { first, second, .. } => first
                .thread_for_pane(target_pane_id)
                .or_else(|| second.thread_for_pane(target_pane_id)),
        }
    }

    fn set_thread_for_pane(&mut self, target_pane_id: PaneId, thread_id: T) -> bool {
        match self {
            SplitPaneNode::Leaf {
                pane_id,
                thread_id: existing,
            } => {
                if *pane_id != target_pane_id {
                    return false;
                }
                *existing = thread_id;
                true
            }
            SplitPaneNode::Split { first, second, .. } => {
                first.set_thread_for_pane(target_pane_id, thread_id)
                    || second.set_thread_for_pane(target_pane_id, thread_id)
            }
        }
    }

    fn leaf_count(&self) -> usize {
        match self {
            SplitPaneNode::Leaf { .. } => 1,
            SplitPaneNode::Split { first, second, .. } => first.leaf_count() + second.leaf_count(),
        }
    }

    fn contains_thread(&self, thread_id: T) -> bool {
        match self {
            SplitPaneNode::Leaf {
                thread_id: existing,
                ..
            } => *existing == thread_id,
            SplitPaneNode::Split { first, second, .. } => {
                first.contains_thread(thread_id) || second.contains_thread(thread_id)
            }
        }
    }

    fn pane_for_thread(&self, thread_id: T) -> Option<PaneId> {
        match self {
            SplitPaneNode::Leaf {
                pane_id,
                thread_id: existing,
            } => (*existing == thread_id).then_some(*pane_id),
            SplitPaneNode::Split { first, second, .. } => first
                .pane_for_thread(thread_id)
                .or_else(|| second.pane_for_thread(thread_id)),
        }
    }

    fn collect_leaves(&self, leaves: &mut Vec<(PaneId, T)>) {
        match self {
            SplitPaneNode::Leaf { pane_id, thread_id } => leaves.push((*pane_id, *thread_id)),
            SplitPaneNode::Split { first, second, .. } => {
                first.collect_leaves(leaves);
                second.collect_leaves(leaves);
            }
        }
    }

    fn pane_area(&self, target_pane_id: PaneId, area: Rect) -> Option<Rect> {
        match self {
            SplitPaneNode::Leaf { pane_id, .. } => (*pane_id == target_pane_id).then_some(area),
            SplitPaneNode::Split {
                axis,
                first,
                second,
            } => {
                let (first_area, _, second_area) = split_regions(area, *axis);
                first
                    .pane_area(target_pane_id, first_area)
                    .or_else(|| second.pane_area(target_pane_id, second_area))
            }
        }
    }

    fn without_thread(self, thread_id: T) -> Option<Self> {
        match self {
            SplitPaneNode::Leaf {
                thread_id: existing,
                ..
            } if existing == thread_id => None,
            SplitPaneNode::Leaf { .. } => Some(self),
            SplitPaneNode::Split {
                axis,
                first,
                second,
            } => match (
                first.without_thread(thread_id),
                second.without_thread(thread_id),
            ) {
                (Some(first), Some(second)) => Some(SplitPaneNode::Split {
                    axis,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (Some(node), None) | (None, Some(node)) => Some(node),
                (None, None) => None,
            },
        }
    }

    fn contains_pane(&self, target_pane_id: PaneId) -> bool {
        match self {
            SplitPaneNode::Leaf { pane_id, .. } => *pane_id == target_pane_id,
            SplitPaneNode::Split { first, second, .. } => {
                first.contains_pane(target_pane_id) || second.contains_pane(target_pane_id)
            }
        }
    }

    fn first_pane_id(&self) -> Option<PaneId> {
        match self {
            SplitPaneNode::Leaf { pane_id, .. } => Some(*pane_id),
            SplitPaneNode::Split { first, .. } => first.first_pane_id(),
        }
    }

    fn visit_leaf_areas<F>(&self, area: Rect, visit: &mut F)
    where
        F: FnMut(PaneId, T, Rect),
    {
        match self {
            SplitPaneNode::Leaf { pane_id, thread_id } => visit(*pane_id, *thread_id, area),
            SplitPaneNode::Split {
                axis,
                first,
                second,
            } => {
                let (first_area, _, second_area) = split_regions(area, *axis);
                first.visit_leaf_areas(first_area, visit);
                second.visit_leaf_areas(second_area, visit);
            }
        }
    }

    fn render_dividers(&self, area: Rect, buf: &mut Buffer, active_pane_id: PaneId) -> bool {
        match self {
            SplitPaneNode::Leaf { pane_id, .. } => *pane_id == active_pane_id,
            SplitPaneNode::Split {
                axis,
                first,
                second,
            } => {
                let (first_area, divider_area, second_area) = split_regions(area, *axis);
                let first_active = first.render_dividers(first_area, buf, active_pane_id);
                let second_active = second.render_dividers(second_area, buf, active_pane_id);
                render_divider(divider_area, buf, *axis, first_active, second_active);
                first_active || second_active
            }
        }
    }
}

fn split_regions(area: Rect, axis: SplitAxis) -> (Rect, Rect, Rect) {
    let areas = match axis {
        SplitAxis::Horizontal => Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(PANE_DIVIDER_THICKNESS),
            Constraint::Fill(1),
        ])
        .split(area),
        SplitAxis::Vertical => Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Length(PANE_DIVIDER_THICKNESS),
            Constraint::Fill(1),
        ])
        .split(area),
    };
    (areas[0], areas[1], areas[2])
}

fn render_divider(
    area: Rect,
    buf: &mut Buffer,
    axis: SplitAxis,
    first_active: bool,
    second_active: bool,
) {
    if area.is_empty() {
        return;
    }

    let style = if first_active || second_active {
        Style::default().cyan().bold()
    } else {
        Style::default().dim()
    };

    match axis {
        SplitAxis::Horizontal => {
            let marker = if first_active {
                "^"
            } else if second_active {
                "v"
            } else {
                "─"
            };
            let marker_x = area.x.saturating_add(area.width / 2);
            for x in area.left()..area.right() {
                let symbol = if x == marker_x { marker } else { "─" };
                buf[(x, area.y)].set_symbol(symbol).set_style(style);
            }
        }
        SplitAxis::Vertical => {
            let marker = if first_active {
                "<"
            } else if second_active {
                ">"
            } else {
                "│"
            };
            let marker_y = area.y.saturating_add(area.height / 2);
            for y in area.top()..area.bottom() {
                let symbol = if y == marker_y { marker } else { "│" };
                buf[(area.x, y)].set_symbol(symbol).set_style(style);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MIN_PANE_HEIGHT;
    use super::MIN_PANE_WIDTH;
    use super::PaneId;
    use super::SplitPaneState;
    use crate::app_event::SplitAxis;
    use insta::assert_snapshot;
    use pretty_assertions::assert_eq;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    fn render_snapshot(state: &SplitPaneState<&'static str>) -> String {
        let area = Rect::new(0, 0, 32, 12);
        let mut buf = Buffer::empty(area);
        state.visit_leaf_areas(area, &mut |_, label, leaf_area| {
            for (offset, ch) in label.chars().enumerate() {
                if offset as u16 >= leaf_area.width {
                    break;
                }
                buf[(leaf_area.x + offset as u16, leaf_area.y)].set_symbol(&ch.to_string());
            }
        });
        state.render_dividers(area, &mut buf);

        let mut lines = Vec::new();
        for row in 0..area.height {
            let mut line = String::new();
            for col in 0..area.width {
                let symbol = buf[(area.x + col, area.y + row)].symbol();
                if symbol.is_empty() {
                    line.push(' ');
                } else {
                    line.push_str(symbol);
                }
            }
            lines.push(line.trim_end().to_string());
        }
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines.join("\n")
    }

    #[test]
    fn split_pane_snapshot() {
        let mut state = SplitPaneState::new("LEFT");
        state.split_active("RIGHT", SplitAxis::Vertical);
        let left_pane_id = state
            .focus_previous()
            .map(|_| state.active_pane_id())
            .expect("left pane");
        assert_eq!(state.focus_pane(left_pane_id), Some("LEFT"));
        state.split_active("BOTTOM", SplitAxis::Horizontal);
        assert_snapshot!("split_pane_render", render_snapshot(&state));
    }

    #[test]
    fn desired_height_expands_to_fill_viewport() {
        assert_eq!(SplitPaneState::<u8>::desired_height(), u16::MAX);
    }

    #[test]
    fn focus_cycles_in_render_order() {
        let mut state = SplitPaneState::new(1_u8);
        state.split_active(2, SplitAxis::Vertical);
        assert_eq!(state.focus_previous(), Some(1));
        assert_eq!(state.focus_next(), Some(2));
    }

    #[test]
    fn remove_thread_collapses_tree() {
        let mut state = SplitPaneState::new(1_u8);
        state.split_active(2, SplitAxis::Vertical);
        state.focus_previous();
        state.split_active(3, SplitAxis::Horizontal);

        let state = state.without_thread(3).expect("tree should remain");
        assert_eq!(state.leaf_count(), 2);
        assert_eq!(state.leaves(), vec![(0, 1), (1, 2)]);
    }

    #[test]
    fn split_refusal_uses_active_pane_size() {
        let mut state = SplitPaneState::new(1_u8);
        state.split_active(2, SplitAxis::Vertical);
        let left_pane_id = state
            .focus_previous()
            .map(|_| state.active_pane_id())
            .expect("left pane");
        state.focus_pane(left_pane_id);
        let narrow_area = Rect::new(
            0,
            0,
            (MIN_PANE_WIDTH * 2).saturating_sub(1),
            MIN_PANE_HEIGHT,
        );
        assert_eq!(
            state.can_split_active(narrow_area, SplitAxis::Vertical),
            false
        );
    }

    #[test]
    fn active_pane_area_tracks_nested_focus() {
        let mut state = SplitPaneState::new(1_u8);
        let right_pane_id = state.split_active(2, SplitAxis::Vertical);
        assert_eq!(right_pane_id, 1);
        state.focus_previous();
        let bottom_pane_id = state.split_active(3, SplitAxis::Horizontal);
        assert_eq!(bottom_pane_id, 2);
        assert_eq!(state.active_pane_id(), 2 as PaneId);
        assert_eq!(
            state.active_pane_area(Rect::new(0, 0, 80, 24)),
            Some(Rect::new(0, 13, 40, 11))
        );
    }
}
