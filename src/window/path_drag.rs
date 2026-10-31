//! Path drags a mod starts (`path.drag`): the app follows the pointer, shows
//! the terminal a release would land in, and drops the paths there. A mod
//! can't: its mouse handlers stop at its own element, so a drag that leaves
//! the panel has no one to follow it, and only the app knows which terminal
//! is under the pointer.
use super::*;

/// A press a mod handed the app, followed by the app from then on.
pub(super) struct PathDrag {
    paths: Vec<PathBuf>,
    /// What the ghost says: the row's name, else the first path's.
    label: String,
    /// The machine the paths live on, when that isn't this one (`space.machine`).
    on: Option<String>,
    down: Point<Pixels>,
    /// Where the pointer is now: the ghost follows it.
    at: Point<Pixels>,
    /// Past the threshold: a drag, not a click.
    moving: bool,
    /// The box a release would land in, and the terminal drawn in it (a
    /// `self.terms` key: a pane, a swapped-in one, or a block's).
    target: Option<(String, String)>,
}

impl PathDrag {
    /// The name a ghost shows for these paths.
    fn label_for(paths: &[PathBuf]) -> String {
        let first = paths.first().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
        match (first, paths.len()) {
            (None, _) => String::new(),
            (Some(name), 0..=1) => name,
            (Some(name), n) => format!("{name} and {} more", n - 1),
        }
    }
}

/// What dropping paths from `on` into the terminal at `address` does.
#[derive(Debug, PartialEq)]
enum Drop {
    /// The paths are already on that terminal's machine: typed in as they are.
    Here,
    /// Local files, a terminal on another machine: copied there first (`hc put`).
    Copy,
    /// Files on another machine, a terminal somewhere else: no way yet.
    Refuse,
}

/// A move counts as a drag once it's this far from the press.
const DRAG_START: f32 = 4.;

/// Movement past the threshold: a press becomes a drag.
fn moved_enough(from: Point<Pixels>, to: Point<Pixels>) -> bool {
    (to.x - from.x).abs().max((to.y - from.y).abs()) >= settings::z(DRAG_START)
}

/// The hit under `at`: the smallest visible box holding the point, so a pane
/// inside a framed tab (or a block inside a column) wins over the container
/// holding it too.
fn pick_target<T>(hits: impl Iterator<Item = (T, Bounds<Pixels>)>, at: Point<Pixels>) -> Option<T> {
    hits.filter(|(_, b)| b.contains(&at))
        .min_by_key(|(_, b)| {
            let (w, h) = (f32::from(b.size.width).max(0.) as i64, f32::from(b.size.height).max(0.) as i64);
            w.saturating_mul(h)
        })
        .map(|(t, _)| t)
}

/// Whether paths from `on` (None: this machine) can go into a terminal at
/// `address` (a remote one has a `<machine>/` prefix), and how.
fn drop_where(on: Option<&str>, address: &str) -> Drop {
    let there = address.split_once('/').map(|(machine, _)| machine);
    match (on, there) {
        (None, None) => Drop::Here,
        (None, Some(_)) => Drop::Copy,
        (Some(from), Some(to)) if from == to => Drop::Here,
        _ => Drop::Refuse,
    }
}

impl AppWindow {
    /// `path.drag`: a mod was pressed on these paths. `at` defaults to the last
    /// press the window saw, so a mod needn't say where it was.
    pub(super) fn path_press(&mut self, args: &Value, cx: &mut Context<Self>) -> Result<Value, String> {
        let paths: Vec<PathBuf> = match args.get("paths") {
            Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).map(PathBuf::from).collect(),
            Some(Value::String(path)) => vec![PathBuf::from(path)],
            _ => return Err("path.drag needs paths: [\"<absolute path>\"]".into()),
        };
        if paths.is_empty() {
            return Err("path.drag needs paths: [\"<absolute path>\"]".into());
        }
        let on = Self::arg(args, "on").filter(|m| !m.is_empty()).map(str::to_owned);
        // A local path must be absolute; one on another machine is that
        // machine's to parse (a remote Linux path isn't absolute to Windows).
        if on.is_none()
            && let Some(relative) = paths.iter().find(|p| !p.is_absolute())
        {
            return Err(format!("path.drag: {} isn't an absolute path", relative.display()));
        }
        let down = match args.get("at") {
            Some(at) if at.get("x").is_some() => point(px(num_in(at, "x")), px(num_in(at, "y"))),
            _ => self.pointer,
        };
        let label = Self::arg(args, "label").filter(|l| !l.is_empty()).map(str::to_owned)
            .unwrap_or_else(|| PathDrag::label_for(&paths));
        self.pane_drag = None;
        self.path_drag = Some(PathDrag { paths, label, on, down, at: down, moving: false, target: None });
        cx.notify();
        Ok(json!({ "ok": true }))
    }

    /// Every terminal on screen now: the box it's drawn in, and the view in it
    /// (a `self.terms` key). `self.terms` also holds the terminals of tabs
    /// nobody shows (so a switch is instant), so it can't be that list alone.
    fn shown_terms(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if self.shows_tab()
            && let Some(tab) = self.model.active_tab()
        {
            for n in tab.shown().visible_blocks() {
                // A pane showing another terminal for now draws that one.
                let swap = format!("{}{SWAP}", n.id);
                let term = if self.swapped.contains_key(&n.id) && self.terms.contains_key(&swap) { swap } else { n.id.clone() };
                if self.terms.contains_key(&term) {
                    out.push((n.id.clone(), term));
                }
            }
        }
        for n in self.chrome.visible_blocks() {
            let term = match n.content() {
                Some(Content::Terminal { .. }) => Some(n.id.clone()),
                // An agent block with a pane draws that pane's view, in the block's box.
                Some(Content::Agent { .. }) => self.shown_agent().map(|a| {
                    self.model.pane_of(&a).filter(|p| self.terms.contains_key(p)).unwrap_or_else(|| n.id.clone())
                }),
                _ => None,
            };
            if let Some(term) = term.filter(|t| self.terms.contains_key(t)) {
                out.push((n.id.clone(), term));
            }
        }
        for n in visible_nodes(&self.chrome) {
            for f in n.floating.iter().filter(|f| !f.hidden) {
                if matches!(&f.content, Content::Terminal { .. }) {
                    let slot = floats::slot(&f.id);
                    if self.terms.contains_key(&slot) {
                        out.push((slot.clone(), slot));
                    }
                }
            }
        }
        for o in &self.overlays {
            if let Some(l) = &o.layout {
                for n in l.visible_blocks() {
                    if matches!(n.content(), Some(Content::Terminal { .. })) && self.terms.contains_key(&n.id) {
                        out.push((n.id.clone(), n.id.clone()));
                    }
                }
            }
        }
        out
    }

    fn path_target(&self, at: Point<Pixels>) -> Option<(String, String)> {
        let bounds = self.bounds.borrow();
        let hits = self.shown_terms().into_iter().filter_map(|(slot, term)| {
            let b = *bounds.get(&slot)?;
            Some(((slot, term), b))
        });
        pick_target(hits, at)
    }

    fn path_move(&mut self, at: Point<Pixels>, pressed: bool, cx: &mut Context<Self>) {
        let (down, moving) = match self.path_drag.as_ref() {
            Some(d) => (d.down, d.moving),
            None => return,
        };
        if !pressed {
            self.cancel_path_drag(cx);
            return;
        }
        if !moving && !moved_enough(down, at) {
            return;
        }
        let target = self.path_target(at);
        if let Some(d) = self.path_drag.as_mut() {
            d.at = at;
            d.moving = true;
            d.target = target;
        }
        cx.notify();
    }

    pub(super) fn cancel_path_drag(&mut self, cx: &mut Context<Self>) {
        if self.path_drag.take().is_some() {
            cx.notify();
        }
    }

    fn path_release(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let target = self.path_target(at);
        let Some(d) = self.path_drag.take() else { return false };
        cx.notify();
        if !d.moving {
            // A click, not a drag: don't eat it, the row's own on_click runs.
            return false;
        }
        if let Some((slot, term)) = target {
            self.drop_paths(&slot, &term, d.paths, d.on.as_deref(), &d.label, window, cx);
        }
        true
    }

    /// Drops `paths` (from `on`, or this machine) into the terminal drawn in
    /// `slot` (its view is `term`), then gives that terminal the keyboard.
    #[allow(clippy::too_many_arguments)]
    fn drop_paths(&mut self, slot: &str, term: &str, paths: Vec<PathBuf>, on: Option<&str>, label: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.terms.get(term) else { return };
        match drop_where(on, &t.address) {
            Drop::Here => t.view.update(cx, |view, _| view.drop_paths(paths)),
            // Local files into a terminal on another machine: copied there first.
            Drop::Copy => t.view.update(cx, |view, cx| view.dropped(paths, cx)),
            Drop::Refuse => {
                let body = format!("{label} is on {}, and dragging doesn't copy files between machines yet.", on.unwrap_or("this machine"));
                let spec = json!({ "kind": "toast", "notification": { "title": "Can't drop that file", "body": body, "level": "warning" } });
                self.open_overlay(mods::new_overlay_id(), spec, window, cx);
                return;
            }
        }
        self.focus_drop(slot, term, window, cx);
    }

    /// The keyboard goes where the files went, the way a click on that pane
    /// would put it there.
    fn focus_drop(&mut self, slot: &str, term: &str, window: &mut Window, cx: &mut Context<Self>) {
        let view = self.terms.get(term);
        let handle = view.map(|t| t.focus.clone());
        let pane = view.and_then(|t| self.model.pane_of(&t.address));
        let (before, before_block) = (self.model.focused_pane(), self.focused_pane.clone());
        if self.model.tab_of(slot).is_some() {
            self.model.focus_pane(slot);
            self.focused_pane = Some(slot.to_owned());
        } else if terminal_block(&self.chrome, Some(slot)).is_some() {
            self.focused_pane = Some(slot.to_owned());
        } else if let Some(pane) = pane {
            // An agent block drawing a pane's view: the model's focus follows.
            self.model.focus_pane(&pane);
            self.focused_pane = None;
        }
        if self.focused_pane != before_block || self.model.focused_pane() != before {
            self.synced_model_pane = self.model.focused_pane();
            self.publish(cx);
            self.save();
        }
        if let Some(handle) = handle {
            handle.focus(window, cx);
        }
        cx.notify();
    }

    /// The ghost under the pointer: what's being dragged, and that it's a drag.
    fn path_ghost(&self) -> Option<AnyElement> {
        let d = self.path_drag.as_ref().filter(|d| d.moving)?;
        let at = d.at;
        Some(div()
            .absolute()
            .left(at.x + settings::z(14.))
            .top(at.y + settings::z(16.))
            .max_w(settings::z(320.))
            .px(settings::z(8.))
            .py(settings::z(4.))
            .rounded(settings::z(6.))
            .bg(c("popover").opacity(0.96))
            .border_1()
            .border_color(c("border"))
            .shadow_md()
            .text_size(settings::z(12.))
            .text_color(c("foreground"))
            .truncate()
            .child(d.label.clone())
            .into_any_element())
    }

    /// The box a release would paste into, in the terminal's color.
    fn path_highlight(&self) -> Option<AnyElement> {
        let (slot, _) = self.path_drag.as_ref().and_then(|d| d.target.clone())?;
        let b = *self.bounds.borrow().get(&slot)?;
        Some(div()
            .absolute()
            .left(b.origin.x)
            .top(b.origin.y)
            .w(b.size.width)
            .h(b.size.height)
            .bg(c("accent").opacity(0.14))
            .border_2()
            .border_color(c("accent"))
            .rounded(settings::z(8.))
            .into_any_element())
    }

    pub(super) fn path_drag_layer(&self, cx: &mut Context<Self>) -> AnyElement {
        let highlight = self.path_highlight();
        let ghost = self.path_ghost();
        let dragging = self.path_drag.as_ref().is_some_and(|d| d.moving);
        let this = cx.entity().downgrade();
        // Always mounted (like the pane grips): a press that never moves still
        // has to end somewhere, and the mod's own handlers stop at its edge.
        // Not `occlude`: the row under the pointer keeps its press, so a press
        // that turns out to be a click still opens the file. The cursor is the
        // window's while dragging, not a hitbox's.
        div()
            .id("path-drag-layer")
            .absolute()
            .inset_0()
            .children(highlight)
            .children(ghost)
            .child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        if dragging {
                            window.set_window_cursor_style(CursorStyle::ClosedHand);
                        }
                        let t = this.clone();
                        window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture {
                                let _ = t.update(cx, |this, cx| {
                                    this.path_move(ev.position, ev.pressed_button == Some(MouseButton::Left), cx)
                                });
                            }
                        });
                        let t = this;
                        window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
                            if phase == DispatchPhase::Capture
                                && ev.button == MouseButton::Left
                                && t.update(cx, |this, cx| this.path_release(ev.position, window, cx)).unwrap_or(false)
                            {
                                cx.stop_propagation();
                            }
                        });
                    },
                )
                .absolute()
                .size_full(),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{Drop, PathDrag, drop_where, moved_enough, pick_target};
    use gpui::{Bounds, point, px, size};
    use std::path::PathBuf;

    fn b(x: f32, y: f32, w: f32, h: f32) -> Bounds<gpui::Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    }

    #[test]
    fn a_press_becomes_a_drag_past_the_threshold() {
        let down = point(px(100.), px(100.));
        assert!(!moved_enough(down, point(px(103.), px(102.))));
        assert!(moved_enough(down, point(px(104.), px(100.))));
        assert!(moved_enough(down, point(px(100.), px(96.))));
    }

    #[test]
    fn the_drop_lands_on_the_smallest_box_holding_the_pointer() {
        let hits = vec![
            ("pane".to_owned(), b(0., 0., 800., 600.)),
            ("block".to_owned(), b(500., 400., 300., 200.)),
            ("elsewhere".to_owned(), b(900., 0., 100., 100.)),
        ];
        assert_eq!(pick_target(hits.clone().into_iter(), point(px(600.), px(450.))).as_deref(), Some("block"));
        assert_eq!(pick_target(hits.clone().into_iter(), point(px(100.), px(100.))).as_deref(), Some("pane"));
        assert_eq!(pick_target(hits.into_iter(), point(px(50.), px(700.))), None);
    }

    #[test]
    fn paths_on_another_machine_only_drop_into_that_machine() {
        assert_eq!(drop_where(None, "t1"), Drop::Here);
        assert_eq!(drop_where(None, "naspc/hS1/t1"), Drop::Copy);
        assert_eq!(drop_where(Some("naspc"), "naspc/hS1/t1"), Drop::Here);
        assert_eq!(drop_where(Some("naspc"), "t1"), Drop::Refuse);
        assert_eq!(drop_where(Some("naspc"), "other/hS1/t1"), Drop::Refuse);
    }

    #[test]
    fn a_ghost_names_what_is_dragged() {
        assert_eq!(PathDrag::label_for(&[PathBuf::from("/tmp/a b.txt")]), "a b.txt");
        assert_eq!(PathDrag::label_for(&[PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]), "a and 1 more");
        assert_eq!(PathDrag::label_for(&[]), "");
    }

    /// The mechanism the layer relies on: a press on a row, followed by the
    /// window-level listener while the pointer is over another element, and a
    /// release that eats the click only because the drag happened.
    #[gpui::test]
    fn a_press_on_a_row_can_become_a_drag_that_ends_outside_it(cx: &mut gpui::TestAppContext) {
        use gpui::{ClickEvent, DispatchPhase, IntoElement, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Render, Window};
        use gpui::prelude::*;
        use std::cell::RefCell;
        use std::rc::Rc;

        #[derive(Default)]
        struct Seen {
            started: bool,
            moved: bool,
            dropped: bool,
            clicked: bool,
        }
        struct Fixture(Rc<RefCell<Seen>>);
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
                let seen = self.0.clone();
                let layer = {
                    let seen = seen.clone();
                    gpui::canvas(
                        |_, _, _| {},
                        move |_, _, window, _| {
                            let s = seen.clone();
                            window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, _| {
                                if phase == DispatchPhase::Capture && ev.pressed_button == Some(MouseButton::Left) {
                                    s.borrow_mut().moved = true;
                                }
                            });
                            let s = seen;
                            window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                                if phase == DispatchPhase::Capture && s.borrow().moved {
                                    s.borrow_mut().dropped = true;
                                    cx.stop_propagation();
                                }
                            });
                        },
                    )
                };
                let down = {
                    let seen = seen.clone();
                    cx.listener(move |_: &mut Fixture, _: &MouseDownEvent, _, _| seen.borrow_mut().started = true)
                };
                let click = {
                    let seen = seen.clone();
                    cx.listener(move |_: &mut Fixture, _: &ClickEvent, _, _| seen.borrow_mut().clicked = true)
                };
                gpui::div()
                    .size_full()
                    .relative()
                    .child(
                        gpui::div()
                            .id("row")
                            .absolute()
                            .left(px(0.))
                            .top(px(0.))
                            .w(px(100.))
                            .h(px(100.))
                            .on_mouse_down(MouseButton::Left, down)
                            .on_click(click),
                    )
                    .child(gpui::div().absolute().left(px(200.)).top(px(0.)).w(px(200.)).h(px(200.)))
                    .child(layer.absolute().size_full())
            }
        }

        let seen = Rc::new(RefCell::new(Seen::default()));
        let (_, visual) = cx.add_window_view(|_, _| Fixture(seen.clone()));
        visual.update(|window, cx| window.draw(cx).clear(cx));
        visual.run_until_parked();
        // A click: press and release on the row, no movement.
        visual.simulate_mouse_down(point(px(50.), px(50.)), MouseButton::Left, Modifiers::none());
        visual.simulate_mouse_up(point(px(50.), px(50.)), MouseButton::Left, Modifiers::none());
        assert!(seen.borrow().started && seen.borrow().clicked, "a press that never moves is a click");
        // A drag: press on the row, move over the terminal, release there.
        seen.borrow_mut().clicked = false;
        visual.update(|window, cx| window.draw(cx).clear(cx));
        visual.simulate_mouse_down(point(px(50.), px(50.)), MouseButton::Left, Modifiers::none());
        visual.simulate_mouse_move(point(px(300.), px(100.)), MouseButton::Left, Modifiers::none());
        visual.simulate_mouse_up(point(px(300.), px(100.)), MouseButton::Left, Modifiers::none());
        assert!(seen.borrow().moved && seen.borrow().dropped, "the window follows a press that leaves the row");
        assert!(!seen.borrow().clicked, "a drag's release isn't a click");
    }
}
