//! The clipboard history: what was copied, newest first, with pins that survive "Clear".
//! Pure data and rules, no UI, so it is tested on its own and saved as JSON.

use std::path::{Path, PathBuf};

use rune::{ClipboardContent, ClipboardImage};
use serde::{Deserialize, Serialize};

/// How many unpinned clips the history keeps; older ones fall off the end.
pub const LIMIT: usize = 200;

#[derive(Clone, Serialize, Deserialize)]
pub struct Clip {
    pub id: u64,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    /// Images stay in memory for the session; they are not written to disk.
    #[serde(skip)]
    pub image: Option<ClipboardImage>,
}

impl Clip {
    pub fn kind(&self) -> &'static str {
        match (&self.image, &self.html) {
            (Some(_), _) => "Image",
            (None, Some(_)) => "Rich text",
            (None, None) => "Text",
        }
    }

    /// One line for the row: the first non-blank line, or the image size.
    pub fn title(&self) -> String {
        if self.text.trim().is_empty()
            && let Some(image) = &self.image {
                let (w, h) = image.size();
                return format!("Image {w} × {h}");
            }
        self.text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string()
    }

    pub fn detail(&self) -> String {
        let chars = self.text.chars().count();
        let lines = self.text.lines().count();
        match (&self.image, chars) {
            (Some(_), 0) => self.kind().to_string(),
            _ if lines > 1 => format!("{} · {chars} chars · {lines} lines", self.kind()),
            _ => format!("{} · {chars} chars", self.kind()),
        }
    }

    pub fn content(&self) -> ClipboardContent {
        let mut c = ClipboardContent::new(self.text.clone());
        if let Some(html) = &self.html { c = c.with_html(html.clone()); }
        if let Some(image) = &self.image { c = c.with_image(image.clone()); }
        c
    }

    fn same(&self, c: &ClipboardContent) -> bool {
        self.text == c.text() && self.html.as_deref() == c.html() && self.image.as_ref() == c.image()
    }

    fn matches(&self, query: &str) -> bool {
        query.is_empty() || self.text.to_lowercase().contains(query)
    }
}

#[derive(Default, Serialize, Deserialize)]
pub struct History {
    pub clips: Vec<Clip>,
    pub next_id: u64,
}

impl History {
    /// Record a copy. A copy already in the history moves to the top instead of repeating.
    /// Returns true when the history changed.
    pub fn capture(&mut self, c: &ClipboardContent) -> bool {
        if c.text().trim().is_empty() && c.image().is_none() { return false; }
        if let Some(i) = self.clips.iter().position(|clip| clip.same(c)) {
            if i == 0 { return false; }
            let clip = self.clips.remove(i);
            self.clips.insert(0, clip);
            return true;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.clips.insert(0, Clip {
            id,
            text: c.text().to_string(),
            html: c.html().map(str::to_string),
            pinned: false,
            image: c.image().cloned(),
        });
        self.trim();
        true
    }

    fn trim(&mut self) {
        let mut unpinned = 0;
        self.clips.retain(|c| c.pinned || { unpinned += 1; unpinned <= LIMIT });
    }

    pub fn get(&self, id: u64) -> Option<&Clip> { self.clips.iter().find(|c| c.id == id) }

    pub fn toggle_pin(&mut self, id: u64) {
        if let Some(c) = self.clips.iter_mut().find(|c| c.id == id) { c.pinned = !c.pinned; }
    }

    pub fn remove(&mut self, id: u64) { self.clips.retain(|c| c.id != id); }

    pub fn clear_unpinned(&mut self) { self.clips.retain(|c| c.pinned); }

    /// Pinned clips first, then the rest, each newest first, filtered by a case-blind search.
    pub fn visible(&self, query: &str) -> Vec<&Clip> {
        let query = query.trim().to_lowercase();
        let hits = || self.clips.iter().filter(|c| c.matches(&query));
        hits().filter(|c| c.pinned).chain(hits().filter(|c| !c.pinned)).collect()
    }

    pub fn load(path: &Path) -> Self {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
        // Clips that are only an image have nothing to write.
        let kept = History {
            clips: self.clips.iter().filter(|c| !c.text.is_empty()).cloned().collect(),
            next_id: self.next_id,
        };
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&kept)?)?;
        std::fs::rename(tmp, path)
    }
}

/// Where the history lives: `$ONECLIPBOARD_HISTORY`, else the platform's per-user data folder.
pub fn default_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ONECLIPBOARD_HISTORY") { return Some(p.into()); }
    let var = |k| std::env::var_os(k).map(PathBuf::from);
    let base = if cfg!(target_os = "windows") {
        var("APPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library/Application Support"))
    } else {
        var("XDG_DATA_HOME").or_else(|| var("HOME").map(|h| h.join(".local/share")))
    };
    base.map(|b| b.join("1Clipboard").join("history.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> ClipboardContent { ClipboardContent::new(s) }

    #[test]
    fn newest_first_and_repeats_move_to_the_top() {
        let mut h = History::default();
        assert!(h.capture(&text("a")));
        assert!(h.capture(&text("b")));
        assert!(!h.capture(&text("b")), "copying the top clip again changes nothing");
        assert!(h.capture(&text("a")));
        let order: Vec<_> = h.clips.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(order, ["a", "b"]);
        assert!(!h.capture(&text("   ")), "blank copies are ignored");
    }

    #[test]
    fn pins_lead_the_list_and_survive_clear_and_the_limit() {
        let mut h = History::default();
        h.capture(&text("keep me"));
        let id = h.clips[0].id;
        h.toggle_pin(id);
        for i in 0..LIMIT + 10 { h.capture(&text(&format!("clip {i}"))); }
        assert_eq!(h.clips.len(), LIMIT + 1);
        assert_eq!(h.visible("")[0].id, id);
        assert_eq!(h.visible("KEEP").len(), 1);
        h.clear_unpinned();
        assert_eq!(h.clips.len(), 1);
    }

    #[test]
    fn saves_and_loads_text_and_html() {
        let dir = std::env::temp_dir().join(format!("oneclipboard-test-{}", std::process::id()));
        let path = dir.join("history.json");
        let mut h = History::default();
        h.capture(&text("plain"));
        h.capture(&ClipboardContent::new("bold").with_html("<b>bold</b>"));
        h.toggle_pin(h.clips[0].id);
        h.save(&path).unwrap();
        let back = History::load(&path);
        assert_eq!(back.clips.len(), 2);
        assert_eq!(back.clips[0].html.as_deref(), Some("<b>bold</b>"));
        assert!(back.clips[0].pinned);
        assert_eq!(back.next_id, 2);
        std::fs::remove_dir_all(dir).ok();
    }
}
