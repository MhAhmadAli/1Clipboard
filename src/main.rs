//! 1Clipboard: a clipboard history manager, written with Rune.
//!
//! It watches the clipboard and keeps what you copy, newest first. Click a clip to put it back on
//! the clipboard. Hover a clip and press Alt+C to copy it, Alt+P to pin or unpin it, Alt+D to
//! delete it. Search the rest as you type. Cmd/Ctrl-V into the window also adds whatever is on
//! the clipboard. The history is saved as JSON in the
//! per-user data folder (see `history::default_path`).
//!
//! `cargo run --release` opens the window; `-- --snap out.png --scene sample` renders it headless.

mod history;

use std::path::PathBuf;

use history::{Clip, History};
use rune::tok::{self, Theme, radius, space, type_};
use rune::{AppSpec, ClipboardContent, Cx, El, Key, KeyEvent, ScrollSpec, TextEdit, Tx, col, row, text, text_input};

/// How often the clipboard is checked for something new, in seconds.
const WATCH_EVERY: f64 = 1.0;

#[derive(Default)]
struct State {
    history: History,
    query: TextEdit,
    /// The clip copied back last, for the "Copied" mark.
    copied: Option<u64>,
    /// The clip under the pointer: the one the Alt shortcuts act on.
    hovered: Option<u64>,
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

    fn delete(&mut self, id: u64) {
        self.edit(|h| h.remove(id));
        if self.copied == Some(id) { self.copied = None; }
        if self.hovered == Some(id) { self.hovered = None; }
    }
}

/// The letter of an Alt chord, by the key pressed rather than what it types (Option-P types π
/// on a Mac). None for anything else, Cmd/Ctrl chords included.
fn alt_letter(ev: &KeyEvent, tx: &Tx) -> Option<char> {
    if !ev.mods.alt || ev.mods.cmd || ev.mods.logo { return None; }
    tx.key_base().or_else(|| match &ev.key {
        Key::Text(t) if t.chars().count() == 1 => t.chars().next(),
        _ => None,
    }).map(|c| c.to_ascii_lowercase())
}

/// Alt+C copies, Alt+P pins or unpins, Alt+D deletes the clip under the pointer.
/// Without a hovered clip the key goes on (to the search field, say).
fn shortcut(s: &mut State, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(id) = s.hovered.filter(|&id| s.history.get(id).is_some()) else { return false };
    match alt_letter(ev, tx) {
        Some('c') => s.copy_back(id, tx),
        Some('p') => s.edit(|h| h.toggle_pin(id)),
        Some('d') => s.delete(id),
        _ => return false,
    }
    true
}

/// How the shortcut modifier reads on this platform.
const ALT: &str = if cfg!(target_os = "macos") { "⌥" } else { "Alt+" };

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

fn clip_row(clip: &Clip, copied: bool, hovered: bool, t: Theme) -> El {
    let id = clip.id;
    let accent = tok::accent(0, true);
    let mut meta = row().gap(space::S2).center_y();
    if clip.pinned { meta = meta.child(text("Pinned").key("pinned").font(type_::LABEL).color(accent)); }
    meta = meta.child(text(clip.detail()).font(type_::LABEL).color(t.text_secondary));
    if copied { meta = meta.child(text("Copied").key("copied").font(type_::LABEL).color(accent)); }

    let body = col().grow(1.0).shrink().min_w(0.0).gap(2.0)
        .child(text(clip.title()).key("title").font(type_::BODY).color(t.text).single_line().truncate())
        .child(meta);

    let row = row().key(format!("clip-{id}")).role("button").label(clip.title())
        .w_full().px(space::S3).py(space::S2).gap(space::S3).center_y()
        .radius(radius::MD).fill(t.card).border(1.0, if copied { accent } else { t.card_border }).hover(t.hover)
        .on_press_tx(move |s: &mut State, tx| s.copy_back(id, tx))
        .on_hover_change(move |s: &mut State, inside, _| {
            if inside { s.hovered = Some(id) } else if s.hovered == Some(id) { s.hovered = None }
        })
        .child(body);
    if !hovered { return row; }
    // The hovered clip shows what its shortcuts do, in place of buttons.
    let pin = if clip.pinned { "unpin" } else { "pin" };
    row.child(text(format!("{ALT}C copy   {ALT}P {pin}   {ALT}D delete")).key("hint")
        .font(type_::LABEL).color(t.text_secondary).single_line())
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
        .on_edit(|s: &mut State| &mut s.query)
        // While a clip is hovered, its shortcuts win over typing into the field.
        .on_input_key(shortcut);

    let body = if clips.is_empty() {
        let msg = if s.history.clips.is_empty() { "Copy something, or press Cmd/Ctrl-V here" } else { "No clips match" };
        col().key("empty").w_full().grow(1.0).center()
            .child(text(msg).font(type_::BODY).color(t.text_secondary))
    } else {
        let mut list = col().key("list").w_full().gap(space::S1);
        for clip in clips { list = list.child(clip_row(clip, s.copied == Some(clip.id), s.hovered == Some(clip.id), t)); }
        list.scroll(ScrollSpec { stick_bottom: false }).shrink().min_h(0.0)
    };

    col().w_full().h_full().padding(space::S6).gap(space::S3).fill(t.bg)
        .on_paste_content(|s: &mut State, content, _| s.capture(content))
        .child(header)
        .child(search)
        .child(body)
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
            ("hover", |s| { sample(s); s.hovered = s.history.clips.first().map(|c| c.id); }),
        ],
        on_key: Some(shortcut),
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
        rt.on_key = Some(shortcut);
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

    fn hover(rt: &mut Runtime<State>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element {key}"));
        rt.pointer_move(x, y);
        settle(rt);
    }

    fn alt(rt: &mut Runtime<State>, letter: &str) {
        rt.key(KeyEvent { key: Key::Text(letter.into()), mods: rune::Mods { alt: true, ..Default::default() } });
        settle(rt);
    }

    #[test]
    fn alt_shortcuts_act_on_the_hovered_clip() {
        let board = clipboard::memory();
        let mut rt = runtime(&board);
        board.set_content(ClipboardContent::new("from another app"));
        tap(&mut rt, "capture");
        rt.paste_content(ClipboardContent::new("pasted"));
        settle(&mut rt);
        assert_eq!(rt.state.history.clips.len(), 2);
        let id = rt.state.history.clips[1].id;

        alt(&mut rt, "p");
        assert!(rt.state.history.clips.iter().all(|c| !c.pinned), "no hovered clip, no pin");

        hover(&mut rt, &format!("clip-{id}"));
        assert_eq!(rt.state.hovered, Some(id));
        assert!(rt.find(&format!("clip-{id}/hint")).is_some(), "the hovered row shows its shortcuts");
        alt(&mut rt, "p");
        assert!(rt.state.history.get(id).unwrap().pinned, "Alt+P pins");
        hover(&mut rt, &format!("clip-{id}"));
        alt(&mut rt, "P");
        assert!(!rt.state.history.get(id).unwrap().pinned, "Alt+P again unpins");
        // Pinning moves rows, so point at the clip again rather than at whatever slid under.
        hover(&mut rt, &format!("clip-{id}"));
        alt(&mut rt, "p");

        tap(&mut rt, "clear");
        assert_eq!(rt.state.history.clips.len(), 1, "Clear keeps pins");
        hover(&mut rt, &format!("clip-{id}"));
        alt(&mut rt, "c");
        assert_eq!(board.content(), Some(ClipboardContent::new("from another app")), "Alt+C copies");

        alt(&mut rt, "d");
        assert!(rt.state.history.clips.is_empty(), "Alt+D deletes");
        assert!(rt.find("empty").is_some());
        for _ in 0..120 { rt.frame(1.0 / 60.0); }
        assert!(rt.state.history.clips.is_empty(), "the watcher doesn't bring back a deleted clip");
    }

    #[test]
    fn shortcuts_win_over_the_search_field_only_while_a_clip_is_hovered() {
        let board = clipboard::memory();
        let mut rt = runtime(&board);
        rt.paste_content(ClipboardContent::new("pin me"));
        settle(&mut rt);
        let id = rt.state.history.clips[0].id;
        tap(&mut rt, "search");
        rt.pointer_move(1.0, 1.0);
        settle(&mut rt);
        rt.key(KeyEvent { key: Key::Text("p".into()), mods: rune::Mods::default() });
        settle(&mut rt);
        assert_eq!(rt.state.query.text, "p", "plain typing still searches");
        rt.update(|s, _| s.query.replace_text(""));
        settle(&mut rt);

        hover(&mut rt, &format!("clip-{id}"));
        alt(&mut rt, "p");
        assert!(rt.state.history.get(id).unwrap().pinned);
        assert_eq!(rt.state.query.text, "", "the shortcut didn't type into the field");
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
