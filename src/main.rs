//! 1Clipboard: a clipboard history manager, written with Rune.
//!
//! It watches the clipboard and keeps what you copy, newest first. Click a clip (or "Copy") to
//! put it back on the clipboard, pin the ones you want to keep, and search the rest. Cmd/Ctrl-V
//! into the window also adds whatever is on the clipboard. The history is saved as JSON in the
//! per-user data folder (see `history::default_path`).
//!
//! `cargo run --release` opens the window; `-- --snap out.png --scene sample` renders it headless.

mod history;

use std::path::PathBuf;

use history::{Clip, History};
use rune::tok::{self, Theme, radius, space, type_};
use rune::{AppSpec, ClipboardContent, Cx, El, ScrollSpec, TextEdit, Tx, col, el, row, text, text_input};

/// How often the clipboard is checked for something new, in seconds.
const WATCH_EVERY: f64 = 1.0;

#[derive(Default)]
struct State {
    history: History,
    query: TextEdit,
    /// The clip copied back last, for the "Copied" mark.
    copied: Option<u64>,
    /// Where the history is saved; None in headless runs and tests, so they never touch it.
    store: Option<PathBuf>,
    /// Clock time of the next clipboard check.
    next_watch: f64,
    /// What the watcher saw last, so a clip deleted while it's still on the clipboard stays gone.
    last_seen: Option<ClipboardContent>,
}

impl State {
    fn load() -> Self {
        let store = if rune::tooling::headless_run().is_some() { None } else { history::default_path() };
        let history = store.as_deref().map(History::load).unwrap_or_default();
        State { history, store, ..State::default() }
    }

    fn save(&self) {
        if let Some(path) = &self.store
            && let Err(e) = self.history.save(path) { eprintln!("1Clipboard: couldn't save {}: {e}", path.display()); }
    }

    fn capture(&mut self, content: &ClipboardContent) {
        if self.history.capture(content) {
            self.save();
            // A new copy from another app isn't the one we put there.
            if self.copied.is_some_and(|id| self.history.clips.first().is_none_or(|c| c.id != id)) {
                self.copied = None;
            }
        }
    }

    /// The watcher's read: only a clipboard that changed since the last look is a new copy.
    fn watched(&mut self, content: &ClipboardContent) {
        if self.last_seen.as_ref() == Some(content) { return; }
        self.last_seen = Some(content.clone());
        self.capture(content);
    }

    fn copy_back(&mut self, id: u64, tx: &mut Tx) {
        if let Some(clip) = self.history.get(id) {
            tx.copy_rich(clip.content());
            self.copied = Some(id);
        }
    }

    fn edit(&mut self, f: impl FnOnce(&mut History)) {
        f(&mut self.history);
        self.save();
    }
}

/// Check the clipboard once a second; a read that finds something new adds it.
fn poll(s: &mut State, clock: f64, tx: &mut Tx) -> bool {
    if clock >= s.next_watch {
        s.next_watch = clock + WATCH_EVERY;
        tx.read_clipboard(|s: &mut State, content, _| s.watched(content));
    }
    tx.wake_at(s.next_watch);
    false
}

fn button(key: impl Into<String>, label: &str, t: Theme, f: impl Fn(&mut State, &mut Tx) + 'static) -> El {
    row().key(key.into()).role("button").label(label).focusable(true)
        .px(space::S2).py(space::S1).radius(radius::SM)
        .fill(t.control_off).hover(t.control_track)
        .child(text(label).font(type_::LABEL).color(t.text))
        .on_press_tx(f)
}

fn clip_row(clip: &Clip, copied: bool, t: Theme) -> El {
    let id = clip.id;
    let accent = tok::accent(0, true);
    let mut meta = row().gap(space::S2).center_y();
    if clip.pinned { meta = meta.child(text("Pinned").key("pinned").font(type_::LABEL).color(accent)); }
    meta = meta.child(text(clip.detail()).font(type_::LABEL).color(t.text_secondary));
    if copied { meta = meta.child(text("Copied").key("copied").font(type_::LABEL).color(accent)); }

    let body = col().grow(1.0).shrink().min_w(0.0).gap(2.0)
        .child(text(clip.title()).key("title").font(type_::BODY).color(t.text).single_line().truncate())
        .child(meta);
    let actions = row().gap(space::S1).center_y()
        .child(button("copy", "Copy", t, move |s, tx| s.copy_back(id, tx)))
        .child(button("pin", if clip.pinned { "Unpin" } else { "Pin" }, t, move |s, _| s.edit(|h| h.toggle_pin(id))))
        .child(button("delete", "Delete", t, move |s, _| {
            s.edit(|h| h.remove(id));
            if s.copied == Some(id) { s.copied = None; }
        }));

    row().key(format!("clip-{id}")).role("button").label(clip.title())
        .w_full().px(space::S3).py(space::S2).gap(space::S3).center_y()
        .radius(radius::MD).fill(t.card).border(1.0, if copied { accent } else { t.card_border }).hover(t.hover)
        .on_press_tx(move |s: &mut State, tx| s.copy_back(id, tx))
        .child(body).child(actions)
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let t = tok::theme(true);
    let clips = s.history.visible(&s.query.text);

    let header = row().w_full().gap(space::S2).center_y()
        .child(text("1Clipboard").key("app-title").font(type_::TITLE).color(t.text).grow(1.0))
        .child(text(format!("{} clips", s.history.clips.len())).key("count").font(type_::LABEL).color(t.text_secondary))
        .child(button("capture", "Add clipboard", t, |_, tx| {
            tx.read_clipboard(|s: &mut State, content, _| s.capture(content));
        }))
        .child(button("clear", "Clear", t, |s, _| s.edit(History::clear_unpinned)));

    let search = text_input(&s.query, type_::BODY).key("search").single_line()
        .placeholder("Search clips", t.text_secondary)
        .w_full().px(space::S3).py(space::S2).radius(radius::MD)
        .fill(t.card).border(1.0, t.card_border).color(t.text)
        .on_edit(|s: &mut State| &mut s.query);

    let body = if clips.is_empty() {
        let msg = if s.history.clips.is_empty() { "Copy something, or press Cmd/Ctrl-V here" } else { "No clips match" };
        col().key("empty").w_full().grow(1.0).center()
            .child(text(msg).font(type_::BODY).color(t.text_secondary))
    } else {
        let mut list = col().key("list").w_full().gap(space::S1);
        for clip in clips { list = list.child(clip_row(clip, s.copied == Some(clip.id), t)); }
        list.scroll(ScrollSpec { stick_bottom: false }).shrink().min_h(0.0)
    };

    col().w_full().h_full().padding(space::S6).gap(space::S3).fill(t.bg)
        .on_paste_content(|s: &mut State, content, _| s.capture(content))
        .child(header)
        .child(search)
        .child(body)
        .child(el().w_full().h(0.0))
}

fn sample(s: &mut State) {
    for c in [
        ClipboardContent::new("https://github.com/anomalyco/rune"),
        ClipboardContent::new("fn main() {\n    println!(\"Hello, clipboard\");\n}"),
        ClipboardContent::new("Meeting notes: ship 1Clipboard on Rune").with_html("<b>Meeting notes</b>: ship 1Clipboard on Rune"),
        ClipboardContent::new("hello@example.com"),
        ClipboardContent::new("The quick brown fox jumps over the lazy dog, again and again, until the line is far too long for one row"),
    ] {
        s.history.capture(&c);
    }
    let id = s.history.clips[3].id;
    s.history.toggle_pin(id);
}

fn main() {
    rune::run(AppSpec::<State> {
        min_size: Some((420.0, 360.0)),
        poll: Some(poll),
        scenes: &[
            ("sample", sample),
            ("search", |s| { sample(s); s.query = TextEdit::new("rune"); }),
            ("copied", |s| { sample(s); s.copied = s.history.clips.first().map(|c| c.id); }),
        ],
        ..AppSpec::new("1Clipboard", (560.0, 520.0), State::load, view)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::{Env, Runtime, clipboard};

    fn runtime(board: &clipboard::MemoryClipboard) -> Runtime<State> {
        let mut rt = Runtime::new(State::default(), view, (560.0, 520.0), Env::new(true, 2.0));
        rt.use_clipboard(board);
        rt.poll = Some(poll);
        settle(&mut rt);
        rt
    }

    /// Let rows finish springing in, so taps land where they rest.
    fn settle(rt: &mut Runtime<State>) {
        for _ in 0..60 { rt.frame(1.0 / 60.0); }
    }

    fn tap(rt: &mut Runtime<State>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        settle(rt);
    }

    #[test]
    fn a_paste_adds_a_clip_and_a_tap_copies_it_back() {
        let board = clipboard::memory();
        let mut rt = runtime(&board);
        rt.paste_content(ClipboardContent::new("first"));
        settle(&mut rt);
        rt.paste_content(ClipboardContent::new("second").with_html("<i>second</i>"));
        settle(&mut rt);
        assert_eq!(rt.state.history.clips.len(), 2);
        assert_eq!(rt.state.history.clips[0].text, "second");

        let first = rt.state.history.clips[1].id;
        tap(&mut rt, &format!("clip-{first}"));
        assert_eq!(board.content(), Some(ClipboardContent::new("first")));
        assert_eq!(rt.state.copied, Some(first));
        assert!(rt.find(&format!("clip-{first}/copied")).is_some());
    }

    #[test]
    fn add_clipboard_reads_it_and_pin_and_delete_work_from_the_row() {
        let board = clipboard::memory();
        let mut rt = runtime(&board);
        board.set_content(ClipboardContent::new("from another app"));
        tap(&mut rt, "capture");
        assert_eq!(rt.state.history.clips.len(), 1);
        let id = rt.state.history.clips[0].id;

        tap(&mut rt, &format!("clip-{id}/pin"));
        assert!(rt.state.history.clips[0].pinned);
        tap(&mut rt, "clear");
        assert_eq!(rt.state.history.clips.len(), 1, "Clear keeps pins");
        tap(&mut rt, &format!("clip-{id}/delete"));
        assert!(rt.state.history.clips.is_empty());
        assert!(rt.find("empty").is_some());
        for _ in 0..120 { rt.frame(1.0 / 60.0); }
        assert!(rt.state.history.clips.is_empty(), "the watcher doesn't bring back a deleted clip");
    }

    #[test]
    fn the_watcher_picks_up_new_copies() {
        let board = clipboard::memory();
        let mut rt = runtime(&board);
        board.set_content(ClipboardContent::new("copied elsewhere"));
        for _ in 0..90 { rt.frame(1.0 / 60.0); }
        assert_eq!(rt.state.history.clips.first().map(|c| c.text.as_str()), Some("copied elsewhere"));
    }
}
