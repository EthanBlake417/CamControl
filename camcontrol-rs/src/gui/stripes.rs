//! PyCharm-style panel stripes (tool_stripes.py): a thin bar along the left,
//! right and bottom edges with one button per panel.
//! - Click a panel's button to open it, or to minimize it. Each edge shows
//!   one panel at a time: opening one minimizes the others on that edge.
//! - Drag a button to another stripe to move the panel to that edge, or along
//!   its stripe to change the order. While dragging, all three stripes show.
//! - Stripes with no buttons are hidden.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use serde::{Deserialize, Serialize};

pub const STRIPE: f32 = 24.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PanelId {
    Controls,
    Captures,
    Measurements,
    #[serde(alias = "Counting")]
    Alignment,
    Fiber,
    FiberLog,
    #[serde(alias = "Histogram")]
    FiberCount,
}

impl PanelId {
    #[cfg(feature = "extras")]
    pub const ALL: [PanelId; 7] = [
        PanelId::Controls,
        PanelId::Measurements,
        PanelId::Captures,
        PanelId::Alignment,
        PanelId::Fiber,
        PanelId::FiberLog,
        PanelId::FiberCount,
    ];
    /// Without the "extras" feature there's no Measurements panel.
    #[cfg(not(feature = "extras"))]
    pub const ALL: [PanelId; 6] = [
        PanelId::Controls,
        PanelId::Captures,
        PanelId::Alignment,
        PanelId::Fiber,
        PanelId::FiberLog,
        PanelId::FiberCount,
    ];

    pub fn title(self) -> &'static str {
        match self {
            PanelId::Controls => "Controls",
            PanelId::Captures => "Captures",
            PanelId::Measurements => "Measurements",
            PanelId::Alignment => "Alignment",
            PanelId::Fiber => "Fiber axis",
            PanelId::FiberLog => "Fiber log",
            PanelId::FiberCount => "Fiber count",
        }
    }

    /// Ctrl+1..5 as in the Python app, plus Ctrl+6 and 7.
    pub fn shortcut(self) -> usize {
        match self {
            PanelId::Controls => 1,
            PanelId::Measurements => 2,
            PanelId::Captures => 3,
            PanelId::Alignment => 4,
            PanelId::Fiber => 5,
            PanelId::FiberLog => 6,
            PanelId::FiberCount => 7,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Edge {
    Left,
    Right,
    Bottom,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Slot {
    pub id: PanelId,
    pub edge: Edge,
    pub open: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PanelLayout {
    /// In stripe order.
    pub slots: Vec<Slot>,
    #[serde(skip)]
    dragging: Option<PanelId>,
    #[serde(skip)]
    stripe_rects: Vec<(Edge, Rect)>,
}

impl Default for PanelLayout {
    fn default() -> Self {
        let s = |id, edge, open| Slot { id, edge, open };
        Self {
            slots: vec![
                s(PanelId::Captures, Edge::Left, true),
                s(PanelId::Controls, Edge::Right, true),
                #[cfg(feature = "extras")]
                s(PanelId::Measurements, Edge::Bottom, true),
                s(PanelId::Alignment, Edge::Bottom, !cfg!(feature = "extras")),
                s(PanelId::Fiber, Edge::Bottom, false),
                s(PanelId::FiberLog, Edge::Bottom, false),
                s(PanelId::FiberCount, Edge::Bottom, false),
            ],
            dragging: None,
            stripe_rects: Vec::new(),
        }
    }
}

impl PanelLayout {
    /// Every panel exactly once, at most one open per edge (after loading saved settings).
    pub fn repair(&mut self) {
        let mut seen = Vec::new();
        self.slots.retain(|s| PanelId::ALL.contains(&s.id)); // panels left out of this build
        self.slots.retain(|s| {
            let new = !seen.contains(&s.id);
            seen.push(s.id);
            new
        });
        for d in PanelLayout::default().slots {
            if !self.slots.iter().any(|s| s.id == d.id) {
                self.slots.push(Slot { open: false, ..d });
            }
        }
        for edge in [Edge::Left, Edge::Right, Edge::Bottom] {
            let mut found = false;
            for s in self.slots.iter_mut().filter(|s| s.edge == edge) {
                if s.open && found {
                    s.open = false;
                }
                found |= s.open;
            }
        }
    }

    pub fn open_on(&self, edge: Edge) -> Option<PanelId> {
        self.slots.iter().find(|s| s.edge == edge && s.open).map(|s| s.id)
    }

    pub fn is_open(&self, id: PanelId) -> bool {
        self.slots.iter().any(|s| s.id == id && s.open)
    }

    pub fn set_open(&mut self, id: PanelId, open: bool) {
        let edge = self.slots.iter().find(|s| s.id == id).map(|s| s.edge);
        for s in &mut self.slots {
            if s.id == id {
                s.open = open;
            } else if open && Some(s.edge) == edge {
                s.open = false; // one open panel per edge
            }
        }
    }

    pub fn toggle(&mut self, id: PanelId) {
        let open = self.is_open(id);
        self.set_open(id, !open);
    }

    fn move_to(&mut self, id: PanelId, edge: Edge, before: Option<PanelId>) {
        let Some(i) = self.slots.iter().position(|s| s.id == id) else { return };
        let mut slot = self.slots.remove(i);
        let was_open = slot.open;
        slot.edge = edge;
        let at = before.and_then(|b| self.slots.iter().position(|s| s.id == b)).unwrap_or(self.slots.len());
        self.slots.insert(at, slot);
        if was_open {
            self.set_open(id, true);
        }
    }

    pub fn stripe_visible(&self, edge: Edge) -> bool {
        self.dragging.is_some() || self.slots.iter().any(|s| s.edge == edge)
    }

    /// Draw one stripe's buttons.
    pub fn stripe(&mut self, ui: &mut egui::Ui, edge: Edge) {
        let rect = ui.max_rect();
        self.stripe_rects.retain(|r| r.0 != edge);
        self.stripe_rects.push((edge, rect));
        let vertical = edge != Edge::Bottom;
        let ids: Vec<(PanelId, bool)> = self.slots.iter().filter(|s| s.edge == edge).map(|s| (s.id, s.open)).collect();
        let mut pos = rect.min + Vec2::new(2.0, 4.0);
        if !vertical {
            pos = rect.min + Vec2::new(4.0, 2.0);
        }
        let mut drop_before = None;
        let pointer = ui.input(|i| i.pointer.hover_pos());
        for (id, open) in ids {
            let color = if open { ui.visuals().strong_text_color() } else { ui.visuals().text_color() };
            let galley = ui.painter().layout_no_wrap(id.title().into(), FontId::proportional(13.0), color);
            let len = galley.size().x + 20.0;
            let size = if vertical { Vec2::new(STRIPE - 4.0, len) } else { Vec2::new(len, STRIPE - 4.0) };
            let r = Rect::from_min_size(pos, size);
            let resp = ui.interact(r, ui.id().with(("stripe", id)), Sense::click_and_drag());
            let accent = ui.visuals().selection.bg_fill;
            if open {
                ui.painter().rect(r, 3.0, accent.gamma_multiply(0.45), Stroke::new(1.0, accent), egui::StrokeKind::Inside);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, 3.0, ui.visuals().widgets.hovered.weak_bg_fill);
            }
            let text_pos = match edge {
                Edge::Left => Pos2::new(r.center().x - galley.size().y / 2.0, r.bottom() - 10.0),
                Edge::Right => Pos2::new(r.center().x + galley.size().y / 2.0, r.top() + 10.0),
                Edge::Bottom => Pos2::new(r.left() + 10.0, r.center().y - galley.size().y / 2.0),
            };
            let angle = match edge {
                Edge::Left => -std::f32::consts::FRAC_PI_2, // reads bottom to top, as in PyCharm
                Edge::Right => std::f32::consts::FRAC_PI_2,
                Edge::Bottom => 0.0,
            };
            ui.painter().add(egui::epaint::TextShape::new(text_pos, galley, color).with_angle(angle));
            let resp = resp.on_hover_text(format!("Ctrl+{}: show / minimize. Drag to another edge to move it.", id.shortcut()));
            if resp.clicked() {
                self.toggle(id);
            }
            if resp.drag_started() {
                self.dragging = Some(id);
            }
            if self.dragging.is_some() && self.dragging != Some(id) {
                if let Some(p) = pointer {
                    let mid = if vertical { r.center().y } else { r.center().x };
                    let along = if vertical { p.y } else { p.x };
                    if rect.contains(p) && along < mid && drop_before.is_none() {
                        drop_before = Some(id);
                    }
                }
            }
            if vertical { pos.y += len + 4.0 } else { pos.x += len + 4.0 }
        }
        if self.dragging.is_some() {
            if let Some(p) = pointer {
                if rect.contains(p) {
                    ui.painter().rect_stroke(rect.shrink(1.0), 2.0, Stroke::new(2.0, ui.visuals().selection.bg_fill), egui::StrokeKind::Inside);
                }
            }
        }
        // Drop: the pointer let go over this stripe.
        if let Some(id) = self.dragging {
            let (released, p) = ui.input(|i| (i.pointer.any_released(), i.pointer.interact_pos()));
            if released {
                if let Some(p) = p {
                    if rect.contains(p) {
                        self.move_to(id, edge, drop_before);
                        self.dragging = None;
                    }
                }
            }
        }
    }

    /// Call once per frame after the stripes: ends a drag that wasn't dropped on a stripe.
    pub fn end_frame(&mut self, ctx: &egui::Context) {
        if self.dragging.is_some() {
            if ctx.input(|i| i.pointer.any_released()) || !ctx.input(|i| i.pointer.any_down()) {
                self.dragging = None;
            }
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
            if let (Some(id), Some(p)) = (self.dragging, ctx.pointer_hover_pos()) {
                let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("stripe_drag")));
                painter.text(p + Vec2::new(12.0, 12.0), Align2::LEFT_TOP, id.title(), FontId::proportional(13.0), Color32::WHITE);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_open_per_edge_and_moving() {
        let mut l = PanelLayout::default();
        l.toggle(PanelId::FiberLog);
        assert!(l.is_open(PanelId::FiberLog) && l.open_on(Edge::Bottom) == Some(PanelId::FiberLog), "one open per edge");
        l.move_to(PanelId::FiberLog, Edge::Left, None);
        assert!(l.is_open(PanelId::FiberLog) && !l.is_open(PanelId::Captures));
        assert_eq!(l.open_on(Edge::Bottom), None);
        l.slots.push(Slot { id: PanelId::Fiber, edge: Edge::Right, open: true });
        l.repair();
        assert_eq!(l.slots.len(), PanelId::ALL.len());
    }

    /// Layouts saved before Counting and Histogram were replaced still load.
    #[test]
    fn old_panel_names_load() {
        let v = serde_json::json!({"slots": [{"id": "Counting", "edge": "Bottom", "open": true}, {"id": "Histogram", "edge": "Left", "open": false}]});
        let mut l: PanelLayout = serde_json::from_value(v).unwrap();
        l.repair();
        assert!(l.is_open(PanelId::Alignment));
        assert_eq!(l.slots.len(), PanelId::ALL.len());
    }
}
