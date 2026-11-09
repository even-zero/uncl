//! Turning host mouse events into either *float geometry* or *child input*.
//!
//! This is the split that S0 got wrong: any mouse event while the float was
//! visible was swallowed by drag/resize, so a full-screen app inside the float
//! never saw a click. The rule now is:
//!
//! * `Alt` held → uncl owns the event (move the float).
//! * `Ctrl` held → uncl owns the event (resize the float from the border it
//!   grabbed).
//! * on the 1-cell border → uncl owns the event; a plain drag there moves the
//!   float, a `Ctrl` drag there resizes it.
//! * on the pin control in the border's right end → uncl owns the event (toggle
//!   the pin), so a click there is never read as a corner grab.
//! * inside the content area → the child owns it, encoded for its own protocol.
//! * outside the float → uncl owns it: click-away to hide, or — while the float
//!   is pinned — to give the keyboard to the owner pane beside it — unless a
//!   gesture started inside the child's grid, in which case the child keeps it
//!   until the button comes up. Losing that `Up` is what leaves a selection
//!   stuck.
//!
//! * a drag the child did not ask for, or a `Shift` drag it did → a text
//!   selection ([`Selection`]). The host never sees the mouse, so the host can
//!   never select: a drag over a shell's output has to be uncl's.
//!
//! Everything here is pure geometry plus a little drag state, which keeps the
//! routing decisions testable.

use vt100::Screen;

use crate::app::ui::tenant::Overlay;
use crate::constants::{MIN_HEIGHT, MIN_WIDTH, ResizeEdges};
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use crate::geometry::Rect;

/// What uncl decided to do with a mouse event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseRoute {
    /// The event landed inside the float's content area and must be encoded for
    /// the child, in coordinates relative to that area.
    Child { col: u16, row: u16 },
    /// uncl handled it (geometry, drag) — the child must not see it.
    Local,
    /// The press landed on the float's pin control: flip it.
    TogglePin,
    /// A click outside the float: hide it.
    HideFloat,
}

/// Decide the route for one event, without mutating anything.
pub fn route(overlay: &Overlay, m: MouseEvent) -> MouseRoute {
    let content = overlay.content_rect();
    let inside = content.contains(m.column, m.row);
    let on_float = overlay.rect.contains(m.column, m.row);
    let alt = m.modifiers.contains(KeyModifiers::ALT);
    let ctrl = m.modifiers.contains(KeyModifiers::CONTROL);

    // An in-progress gesture inside the child's grid outranks everything: the
    // child saw the press, so it must see the drag and the release even when the
    // pointer is over the border, over the owner pane, or off-screen (where the
    // coordinates clamp to the nearest cell, as a real terminal reports them).
    if overlay.child_grab
        && matches!(
            m.kind,
            MouseEventKind::Down(_)
                | MouseEventKind::Up(_)
                | MouseEventKind::Drag(_)
                | MouseEventKind::Moved
        )
    {
        let (col, row) = local_point(content, m.column, m.row);
        return MouseRoute::Child { col, row };
    }

    match m.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if alt && on_float {
                MouseRoute::Local
            } else if !alt && !ctrl && overlay
                .pin_cell()
                .is_some_and(|pin| pin.contains(m.column, m.row))
            {
                // The pin control sits on the border, which every other press
                // there reads as a resize grab. Checked before the border rule
                // so a plain click on it flips the pin instead of starting one;
                // ctrl+drag reads it as a resize like any other border press.
                MouseRoute::TogglePin
            } else if on_float && !inside {
                MouseRoute::Local
            } else if inside {
                let (col, row) = local_point(content, m.column, m.row);
                MouseRoute::Child { col, row }
            } else if overlay.pinned {
                // A pinned float keeps its place: the owner pane takes the press
                // instead of the float disappearing.
                MouseRoute::Local
            } else {
                MouseRoute::HideFloat
            }
        }

        // While a local drag is in progress it keeps owning the events, even if
        // the pointer leaves the float.
        MouseEventKind::Drag(_) | MouseEventKind::Up(_) if overlay.dragging || overlay.resizing => {
            MouseRoute::Local
        }

        MouseEventKind::Down(_)
        | MouseEventKind::ScrollUp
        | MouseEventKind::ScrollDown
        | MouseEventKind::ScrollLeft
        | MouseEventKind::ScrollRight => {
            // A ctrl+scroll over the float is uncl's to resize with; anything
            // else over content belongs to the child.
            if inside && !alt && !ctrl {
                let (col, row) = local_point(content, m.column, m.row);
                MouseRoute::Child { col, row }
            } else {
                MouseRoute::Local
            }
        }

        MouseEventKind::Drag(_) | MouseEventKind::Up(_) | MouseEventKind::Moved => {
            // While a local drag or resize owns the pointer it keeps owning the
            // motion, even when it started on the border and the pointer has
            // already travelled over the content area.
            if overlay.dragging || overlay.resizing {
                MouseRoute::Local
            } else if inside && !alt && !ctrl {
                let (col, row) = local_point(content, m.column, m.row);
                MouseRoute::Child { col, row }
            } else {
                MouseRoute::Local
            }
        }
    }
}

/// Convert a screen position into a zero-based cell inside `content`, clamping
/// the pointer to the grid so off-grid drags still land on the nearest cell.
fn local_point(content: Rect, column: u16, row: u16) -> (u16, u16) {
    let max_col = content.right().saturating_sub(1).max(content.x);
    let max_row = content.bottom().saturating_sub(1).max(content.y);
    (
        column.clamp(content.x, max_col) - content.x,
        row.clamp(content.y, max_row) - content.y,
    )
}

/// Which pane a wheel event should scroll, if any.
///
/// The child has first claim: a wheel over content goes to it whenever it asked
/// for mouse reporting (`?1000h` and friends), which is why this is decided in
/// `ui::owner` *after* the encoder declined, not here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Wheel {
    Up,
    Down,
}

impl Wheel {
    pub fn from_kind(kind: MouseEventKind) -> Option<Self> {
        match kind {
            MouseEventKind::ScrollUp => Some(Wheel::Up),
            MouseEventKind::ScrollDown => Some(Wheel::Down),
            _ => None,
        }
    }
}

/// Start a drag, a resize, or nothing, based on where the press landed.
pub fn begin(overlay: &mut Overlay, m: MouseEvent) {
    if m.kind != MouseEventKind::Down(MouseButton::Left) {
        return;
    }

    // The child owns the pointer until it releases; taking it away here would
    // strand the child's drag state.
    if overlay.child_grab {
        return;
    }

    let alt = m.modifiers.contains(KeyModifiers::ALT);
    let ctrl = m.modifiers.contains(KeyModifiers::CONTROL);

    // The pin control is uncl's, not a resize handle: a plain press there is a
    // click on the toggle. `Alt` still drags, as it does everywhere else on the
    // border; `Ctrl` still resizes, so only a plain press toggles.
    if !alt && !ctrl && overlay.pin_cell().is_some_and(|pin| pin.contains(m.column, m.row)) {
        return;
    }

    let content = overlay.content_rect();
    let inside = content.contains(m.column, m.row);

    if alt && overlay.rect.contains(m.column, m.row) {
        overlay.dragging = true;
        overlay.drag_offset = (
            m.column.saturating_sub(overlay.rect.x),
            m.row.saturating_sub(overlay.rect.y),
        );
        return;
    }

    if !overlay.rect.contains(m.column, m.row) {
        // Routed here while the float is pinned and the press fell outside it:
        // that belongs to the owner pane, and there is no float geometry to grab.
        return;
    }

    if ctrl {
        // Ctrl+drag owns the float and turns it into a resize from the border.
        // The anchors say which edges were grabbed: a corner press sets both of
        // its axes, a plain edge press just the one it landed on.
        overlay.resizing = true;
        overlay.resize_anchored = ResizeEdges::from_point(overlay.rect, m.column, m.row);
        return;
    }

    if inside {
        // Plain press inside the content area: the child owns it, so uncl
        // starts no local gesture of its own.
        return;
    }

    // Plain press on the float's border: it only moves the float now; growing
    // and shrinking are the `Ctrl` drag's job.
    overlay.dragging = true;
    overlay.drag_offset = (
        m.column.saturating_sub(overlay.rect.x),
        m.row.saturating_sub(overlay.rect.y),
    );
}

/// Apply an in-progress drag/resize. Returns whether anything changed.
pub fn drag(overlay: &mut Overlay, m: MouseEvent, bounds: (u16, u16)) -> bool {
    let moved = matches!(
        m.kind,
        MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Moved
    );

    if overlay.resizing && moved {
        // Only the grabbed axis moves: an edge drag resizes that one dimension,
        // a corner drag both. The anchors were fixed when the press began, so
        // dragging past an edge keeps resizing the anchored dimension.
        let rect = overlay.rect;
        let mut new_x = rect.x;
        let mut new_y = rect.y;
        let mut new_width = rect.width;
        let mut new_height = rect.height;

        if overlay.resize_anchored.left || overlay.resize_anchored.right {
            if overlay.resize_anchored.right {
                // The right edge is anchored: the width follows the pointer.
                new_width = m.column.saturating_sub(rect.x).max(MIN_WIDTH);
            } else {
                // The left edge is anchored: it follows the pointer.
                new_width = rect.right().saturating_sub(m.column).max(MIN_WIDTH);
                new_x = m.column;
            }
        }

        if overlay.resize_anchored.top || overlay.resize_anchored.bottom {
            if overlay.resize_anchored.bottom {
                // The bottom edge is anchored: the height follows the pointer.
                new_height = m.row.saturating_add(1).saturating_sub(rect.y).max(MIN_HEIGHT);
            } else {
                // The top edge is anchored: it follows the pointer.
                new_height = rect.bottom().saturating_sub(m.row).max(MIN_HEIGHT);
                new_y = m.row;
            }
        }

        overlay.resize_to(new_x, new_y, new_width, new_height, bounds);
        return true;
    }

    if overlay.dragging && moved {
        let max_x = bounds.0.saturating_sub(overlay.rect.width);
        let max_y = bounds.1.saturating_sub(overlay.rect.height);
        overlay.rect.x = m.column.saturating_sub(overlay.drag_offset.0).min(max_x);
        overlay.rect.y = m.row.saturating_sub(overlay.drag_offset.1).min(max_y);
        return true;
    }

    false
}

/// Finish any drag/resize. Returns whether one was in progress.
pub fn end(overlay: &mut Overlay) -> bool {
    let was_active = overlay.dragging || overlay.resizing;
    overlay.dragging = false;
    overlay.resizing = false;
    overlay.resize_anchored = ResizeEdges::default();
    was_active
}

/// A text selection in one pane, in that pane's own visible cells.
///
/// uncl holds the host's mouse, so the host terminal never sees a drag and cannot
/// select anything: the drag arrives here, names a cell, and the text behind that
/// cell is in a `vt100` grid uncl already owns. The selection is therefore uncl's
/// and so is the copy (see `ui::owner`).
///
/// Coordinates are `(row, column)` relative to the pane's grid, and they name the
/// *visible* grid — the snapshot the frame was painted from, scroll offset and
/// all — so what is copied is what was pointed at.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Selection {
    /// Which pane: the float, or the shell behind it.
    pub tenant: bool,
    /// Where the press landed.
    pub anchor: (u16, u16),
    /// Where the pointer is now.
    pub head: (u16, u16),
}

impl Selection {
    pub fn new(tenant: bool, row: u16, col: u16) -> Self {
        Self {
            tenant,
            anchor: (row, col),
            head: (row, col),
        }
    }

    /// Move the loose end, keeping the anchor where the press put it.
    pub fn extend(&mut self, row: u16, col: u16) {
        self.head = (row, col);
    }

    /// Has the pointer left the cell the press landed on?
    ///
    /// A press and its release on one cell is a click — a hyperlink, say — and
    /// not a selection of nothing.
    pub fn dragged(&self) -> bool {
        self.anchor != self.head
    }

    /// The corners, top-left first, so a drag in any direction is the same
    /// selection.
    fn ordered(&self) -> ((u16, u16), (u16, u16)) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// Every row the selection covers, top to bottom.
    pub fn rows(&self) -> std::ops::RangeInclusive<u16> {
        let ((first, _), (last, _)) = self.ordered();
        first..=last
    }

    /// The columns selected on `row`, as a half-open range clamped to `width`.
    ///
    /// `None` on a row the selection does not reach, and for the empty range a
    /// drag that never left its first cell produces.
    pub fn span(&self, row: u16, width: u16) -> Option<std::ops::Range<u16>> {
        let ((first_row, first_col), (last_row, last_col)) = self.ordered();
        if row < first_row || row > last_row {
            return None;
        }

        let from = if row == first_row { first_col } else { 0 };
        // The cell under the pointer is part of the selection, so the end is
        // inclusive and becomes half-open here.
        let to = if row == last_row {
            last_col.saturating_add(1)
        } else {
            width
        };
        let to = to.min(width);

        (from < to).then_some(from..to)
    }

    /// The selected text: rows joined with newlines, trailing blanks dropped.
    ///
    /// A row's tail is padding, not content: a shell's grid is full of blanks, and
    /// a copy that carried them would be full of trailing spaces.
    pub fn text(&self, screen: &Screen) -> String {
        // `size()` is `(rows, columns)`.
        let width = screen.size().1;
        self.rows()
            .filter_map(|row| {
                let cols = self.span(row, width)?;
                let mut line = String::new();
                for col in cols {
                    if let Some(cell) = screen.cell(row, col) {
                        line.push_str(cell.contents());
                    }
                }
                Some(line.trim_end().to_string())
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::{DEFAULT_HEIGHT, DEFAULT_WIDTH};

    fn overlay() -> Overlay {
        Overlay::with_rect(Rect::new(10, 5, DEFAULT_WIDTH, DEFAULT_HEIGHT))
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn alt_mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::ALT,
        }
    }

    fn ctrl_mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::CONTROL,
        }
    }

    #[test]
    fn clicks_inside_the_content_go_to_the_child_in_local_coordinates() {
        let overlay = overlay();
        let content = overlay.content_rect();
        let route = route(&overlay, mouse(MouseEventKind::Down(MouseButton::Left), content.x + 3, content.y + 2));
        assert_eq!(route, MouseRoute::Child { col: 3, row: 2 });
    }

    #[test]
    fn clicks_on_the_border_are_local() {
        let overlay = overlay();
        let event = mouse(MouseEventKind::Down(MouseButton::Left), overlay.rect.x, overlay.rect.y);
        assert_eq!(route(&overlay, event), MouseRoute::Local);
    }

    #[test]
    fn alt_click_inside_moves_the_float_instead_of_reaching_the_child() {
        let overlay = overlay();
        let content = overlay.content_rect();
        let route = route(&overlay, alt_mouse(MouseEventKind::Down(MouseButton::Left), content.x + 1, content.y + 1));
        assert_eq!(route, MouseRoute::Local);
    }

    #[test]
    fn clicks_outside_hide_the_float() {
        let overlay = overlay();
        let route = route(&overlay, mouse(MouseEventKind::Down(MouseButton::Left), 0, 0));
        assert_eq!(route, MouseRoute::HideFloat);
    }

    #[test]
    fn clicks_on_the_pin_toggle_it_instead_of_resizing() {
        let overlay = overlay();
        let pin = overlay.pin_cell().expect("a default float has room for the pin");
        let route = route(
            &overlay,
            mouse(MouseEventKind::Down(MouseButton::Left), pin.x + 1, pin.y),
        );
        assert_eq!(route, MouseRoute::TogglePin);
    }

    #[test]
    fn alt_click_on_the_pin_still_moves_the_float() {
        let overlay = overlay();
        let pin = overlay.pin_cell().expect("a default float has room for the pin");
        // `Alt` drags from anywhere on the border, the pin included.
        let route = route(
            &overlay,
            alt_mouse(MouseEventKind::Down(MouseButton::Left), pin.x + 1, pin.y),
        );
        assert_eq!(route, MouseRoute::Local);
    }

    #[test]
    fn a_pinned_float_stays_when_the_pointer_leaves_it() {
        let mut overlay = overlay();
        overlay.pinned = true;
        let route = route(&overlay, mouse(MouseEventKind::Down(MouseButton::Left), 0, 0));
        // The press belongs to the owner pane, not to a click-away that hides.
        assert_eq!(route, MouseRoute::Local);
    }

    #[test]
    fn begin_does_not_start_a_resize_on_the_pin() {
        let mut overlay = overlay();
        let pin = overlay.pin_cell().expect("a default float has room for the pin");
        // The right-hand cell is the corner, which is the grab `begin` would
        // otherwise take.
        begin(
            &mut overlay,
            mouse(MouseEventKind::Down(MouseButton::Left), pin.x + 2, pin.y),
        );
        assert!(!overlay.resizing, "the pin is not a resize handle");
        assert!(!overlay.dragging);
    }

    #[test]
    fn scroll_inside_reaches_the_child() {
        let overlay = overlay();
        let content = overlay.content_rect();
        let route = route(&overlay, mouse(MouseEventKind::ScrollDown, content.x, content.y));
        assert_eq!(route, MouseRoute::Child { col: 0, row: 0 });
    }

    #[test]
    fn ctrl_scroll_over_the_float_is_local_so_it_can_resize() {
        let overlay = overlay();
        let content = overlay.content_rect();
        let event = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: content.x,
            row: content.y,
            modifiers: KeyModifiers::CONTROL,
        };
        assert_eq!(route(&overlay, event), MouseRoute::Local);
    }

    #[test]
    fn ctrl_scroll_on_the_border_is_local() {
        let overlay = overlay();
        let event = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: overlay.rect.x,
            row: overlay.rect.y,
            modifiers: KeyModifiers::CONTROL,
        };
        assert_eq!(route(&overlay, event), MouseRoute::Local);
    }

    #[test]
    fn drags_inside_are_forwarded_when_no_local_drag_is_active() {
        let overlay = overlay();
        let content = overlay.content_rect();
        let route = route(
            &overlay,
            mouse(MouseEventKind::Drag(MouseButton::Left), content.x + 1, content.y + 1),
        );
        assert_eq!(route, MouseRoute::Child { col: 1, row: 1 });
    }

    #[test]
    fn an_active_local_drag_keeps_the_events_even_outside() {
        let mut overlay = overlay();
        overlay.dragging = true;
        let dragging = route(&overlay, mouse(MouseEventKind::Drag(MouseButton::Left), 0, 0));
        assert_eq!(dragging, MouseRoute::Local);
        let released = route(&overlay, mouse(MouseEventKind::Up(MouseButton::Left), 0, 0));
        assert_eq!(released, MouseRoute::Local);
    }

    #[test]
    fn motion_during_a_local_drag_stays_local_even_over_content() {
        let mut overlay = overlay();
        overlay.dragging = true;
        let content = overlay.content_rect();
        let moving = route(&overlay, mouse(MouseEventKind::Moved, content.x + 1, content.y + 1));
        assert_eq!(moving, MouseRoute::Local);
    }

    #[test]
    fn begin_starts_a_move_on_alt_and_a_resize_on_ctrl() {
        let mut moving = overlay();
        let content = moving.content_rect();
        let press = alt_mouse(MouseEventKind::Down(MouseButton::Left), content.x + 2, content.y + 2);
        begin(&mut moving, press);
        assert!(moving.dragging);

        let mut resized = overlay();
        let corner_x = resized.rect.right() - 1;
        let corner_y = resized.rect.bottom() - 1;
        let press = ctrl_mouse(MouseEventKind::Down(MouseButton::Left), corner_x, corner_y);
        begin(&mut resized, press);
        assert!(resized.resizing);
        assert!(resized.resize_anchored.right && resized.resize_anchored.bottom);
        assert!(!resized.resize_anchored.left && !resized.resize_anchored.top);

        // A plain press on the border moves the float now; only ctrl resizes.
        let mut moved = overlay();
        let press = mouse(MouseEventKind::Down(MouseButton::Left), moved.rect.x, moved.rect.y);
        begin(&mut moved, press);
        assert!(moved.dragging);
        assert!(!moved.resizing);

        // A plain click inside starts nothing locally.
        let mut plain = overlay();
        let content = plain.content_rect();
        let press = mouse(MouseEventKind::Down(MouseButton::Left), content.x + 2, content.y + 2);
        begin(&mut plain, press);
        assert!(!plain.dragging && !plain.resizing);
    }

    #[test]
    fn a_plain_border_drag_moves_the_float_instead_of_resizing() {
        let mut overlay = overlay();
        let before = overlay.rect;
        let press_x = overlay.rect.right() - 1;
        let press_y = overlay.rect.bottom() - 1;
        begin(&mut overlay, mouse(MouseEventKind::Down(MouseButton::Left), press_x, press_y));
        assert!(overlay.dragging);
        assert!(!overlay.resizing);

        let moved = drag(
            &mut overlay,
            mouse(
                MouseEventKind::Drag(MouseButton::Left),
                before.right() + 2,
                before.bottom() + 1,
            ),
            (120, 60),
        );
        assert!(moved);
        assert_eq!(overlay.rect.width, before.width);
        assert_eq!(overlay.rect.height, before.height);
        assert_ne!(overlay.rect, before);
        assert!(!overlay.resizing);
    }

    #[test]
    fn ctrl_drag_on_the_border_resizes_the_float() {
        let mut overlay = overlay();
        let before = overlay.rect;
        let press_x = overlay.rect.right() - 1;
        let press_y = overlay.rect.bottom() - 1;
        begin(
            &mut overlay,
            ctrl_mouse(MouseEventKind::Down(MouseButton::Left), press_x, press_y),
        );
        assert!(overlay.resizing);
        assert!(overlay.resize_anchored.right && overlay.resize_anchored.bottom);

        let resized = drag(
            &mut overlay,
            mouse(
                MouseEventKind::Drag(MouseButton::Left),
                before.right() + 4,
                before.bottom() + 2,
            ),
            (120, 60),
        );
        assert!(resized);
        assert!(overlay.rect.width > before.width);
        assert!(overlay.rect.height > before.height);
    }

    #[test]
    fn dragging_the_bottom_right_corner_grows_the_float() {
        let mut overlay = overlay();
        let before = overlay.rect;
        overlay.resizing = true;
        overlay.resize_anchored = ResizeEdges {
            left: false,
            right: true,
            top: false,
            bottom: true,
        };
        drag(
            &mut overlay,
            mouse(
                MouseEventKind::Drag(MouseButton::Left),
                before.right() + 4,
                before.bottom() + 2,
            ),
            (120, 60),
        );
        assert!(overlay.rect.width > before.width);
        assert!(overlay.rect.height > before.height);
        assert_eq!(overlay.rect.x, before.x);
    }

    #[test]
    fn dragging_the_bottom_edge_only_resizes_vertically() {
        let mut overlay = overlay();
        overlay.resizing = true;
        overlay.resize_anchored = ResizeEdges {
            left: false,
            right: false,
            top: false,
            bottom: true,
        };
        let before = overlay.rect;
        let resized = drag(
            &mut overlay,
            mouse(MouseEventKind::Drag(MouseButton::Left), before.right(), before.bottom() + 2),
            (120, 60),
        );
        assert!(resized);
        // Only the height follows the pointer; the width is untouched.
        assert_eq!(overlay.rect.width, before.width);
        assert!(overlay.rect.height > before.height);
    }

    #[test]
    fn dragging_the_left_edge_only_resizes_horizontally() {
        let mut overlay = overlay();
        overlay.resizing = true;
        overlay.resize_anchored = ResizeEdges {
            left: true,
            right: false,
            top: false,
            bottom: false,
        };
        let before = overlay.rect;
        let resized = drag(
            &mut overlay,
            mouse(MouseEventKind::Drag(MouseButton::Left), before.x - 2, before.bottom() - 1),
            (120, 60),
        );
        assert!(resized);
        // Only the width follows the pointer; the height is untouched.
        assert_eq!(overlay.rect.height, before.height);
        assert!(overlay.rect.width > before.width);
    }

    #[test]
    fn moving_with_alt_keeps_the_size() {
        let mut overlay = overlay();
        let size = (overlay.rect.width, overlay.rect.height);
        overlay.dragging = true;
        overlay.drag_offset = (5, 3);
        drag(
            &mut overlay,
            mouse(MouseEventKind::Drag(MouseButton::Left), 40, 20),
            (200, 80),
        );
        assert_eq!((overlay.rect.width, overlay.rect.height), size);
        assert_eq!(overlay.rect.x, 35);
        assert_eq!(overlay.rect.y, 17);
    }

    #[test]
    fn a_child_gesture_keeps_the_pointer_until_release() {
        let mut overlay = overlay();
        overlay.child_grab = true;

        // Outside the float entirely: still the child's, clamped to the grid.
        let dragged = route(&overlay, mouse(MouseEventKind::Drag(MouseButton::Left), 0, 0));
        assert_eq!(dragged, MouseRoute::Child { col: 0, row: 0 });

        let released = route(&overlay, mouse(MouseEventKind::Up(MouseButton::Left), 0, 0));
        assert_eq!(released, MouseRoute::Child { col: 0, row: 0 });
    }

    #[test]
    fn off_grid_drag_coordinates_clamp_to_the_last_cell() {
        let mut overlay = overlay();
        overlay.child_grab = true;
        let content = overlay.content_rect();

        let route = route(
            &overlay,
            mouse(MouseEventKind::Drag(MouseButton::Left), u16::MAX, u16::MAX),
        );
        assert_eq!(
            route,
            MouseRoute::Child {
                col: content.width - 1,
                row: content.height - 1
            }
        );
    }

    #[test]
    fn scroll_reaches_the_child_during_a_gesture_only_when_inside() {
        let mut overlay = overlay();
        overlay.child_grab = true;
        let content = overlay.content_rect();

        let inside = route(&overlay, mouse(MouseEventKind::ScrollUp, content.x, content.y));
        assert_eq!(inside, MouseRoute::Child { col: 0, row: 0 });

        let outside = route(&overlay, mouse(MouseEventKind::ScrollUp, 0, 0));
        assert_eq!(outside, MouseRoute::Local);
    }

    #[test]
    fn begin_does_nothing_while_the_child_owns_the_pointer() {
        let mut overlay = overlay();
        overlay.child_grab = true;
        let content = overlay.content_rect();

        // Even Alt+drag must not steal an in-flight child gesture...
        begin(&mut overlay, alt_mouse(MouseEventKind::Down(MouseButton::Left), content.x + 1, content.y + 1));
        assert!(!overlay.dragging && !overlay.resizing);

        // ...and neither must a border press.
        let corner_x = overlay.rect.right() - 1;
        let corner_y = overlay.rect.bottom() - 1;
        begin(&mut overlay, mouse(MouseEventKind::Down(MouseButton::Left), corner_x, corner_y));
        assert!(!overlay.resizing);
    }

    #[test]
    fn only_vertical_wheels_scroll() {
        assert_eq!(
            Wheel::from_kind(MouseEventKind::ScrollUp),
            Some(Wheel::Up)
        );
        assert_eq!(
            Wheel::from_kind(MouseEventKind::ScrollDown),
            Some(Wheel::Down)
        );
        assert_eq!(Wheel::from_kind(MouseEventKind::ScrollLeft), None);
        assert_eq!(Wheel::from_kind(MouseEventKind::Moved), None);
    }

    #[test]
    fn end_reports_whether_something_was_active() {
        let mut overlay = overlay();
        assert!(!end(&mut overlay));
        overlay.dragging = true;
        assert!(end(&mut overlay));
        assert!(!overlay.dragging);
    }

    fn select(anchor: (u16, u16), head: (u16, u16)) -> Selection {
        let mut selection = Selection::new(false, anchor.0, anchor.1);
        selection.extend(head.0, head.1);
        selection
    }

    #[test]
    fn a_drag_selects_the_same_cells_from_either_direction() {
        let forwards = select((0, 1), (2, 4));
        let backwards = select((2, 4), (0, 1));
        assert_eq!(forwards, backwards);
        assert_eq!(forwards.rows(), 0..=2);

        // The first row starts where the drag did and runs to the edge; the last
        // one starts at the edge and *includes* the cell under the pointer.
        assert_eq!(forwards.span(0, 20), Some(1..20));
        assert_eq!(forwards.span(1, 20), Some(0..20));
        assert_eq!(forwards.span(2, 20), Some(0..5));
        assert_eq!(forwards.span(3, 20), None);

        // Clamped to the grid, never past it.
        assert_eq!(select((0, 18), (0, 30)).span(0, 20), Some(18..20));
    }

    #[test]
    fn a_press_that_never_moved_is_a_click_not_a_selection() {
        let click = select((4, 4), (4, 4));
        assert!(!click.dragged());
        // Its span is still one cell; the painter is what refuses to shade a click
        // (see `render::Renderer::paint_selection`).
        assert_eq!(click.span(4, 10), Some(4..5));
    }

    #[test]
    fn the_copied_text_is_the_rows_without_their_padding() {
        let mut parser = vt100::Parser::new(
            crate::constants::nz(24),
            crate::constants::nz(20),
            0,
        );
        parser.process(b"hello world   \r\nsecond line\r\nthird");

        let screen = parser.screen();
        // "world" on the first row, all of "second line" on the next.
        assert_eq!(select((0, 6), (1, 20)).text(screen), "world\nsecond line");
        // Trailing blanks go; leading and inner ones stay.
        assert_eq!(select((0, 0), (0, 20)).text(screen), "hello world");
        assert_eq!(select((1, 0), (1, 5)).text(screen), "second");
        // A whole screen with nothing selected on the last row.
        assert_eq!(select((2, 0), (2, 20)).text(screen), "third");
    }
}
