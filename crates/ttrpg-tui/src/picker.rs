//! The audio file picker: a directory browser with multi-select, in the style of
//! yazi. Only folders and audio files are listed. The selection survives moving
//! between folders. No drawing here; see `ui::modals`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::ListState;
use ttrpg_core::audio::{is_audio_file_name, natural_cmp};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Parent,
    Dir,
    Audio,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub kind: Kind,
    pub size: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Keep the picker open.
    Open,
    Cancel,
    Done(Vec<PathBuf>),
}

#[derive(Debug)]
pub struct FilePicker {
    pub dir: PathBuf,
    pub entries: Vec<Entry>,
    pub state: ListState,
    pub selected: BTreeSet<PathBuf>,
    pub show_hidden: bool,
    /// Something the user should read: an unreadable folder, or "select a file first".
    pub message: Option<String>,
    /// Screen rectangles of the last draw, for the mouse.
    pub list_inner: Rect,
    pub done_button: Rect,
    pub cancel_button: Rect,
}

impl FilePicker {
    /// Start in `start` (or its nearest existing parent), with `preselected` ticked.
    pub fn new(start: &Path, preselected: &[PathBuf]) -> Self {
        let mut dir = start.to_path_buf();
        while !dir.is_dir() {
            if !dir.pop() {
                dir = PathBuf::from("/");
                break;
            }
        }
        let mut p = FilePicker {
            dir,
            entries: Vec::new(),
            state: ListState::default(),
            selected: preselected.iter().cloned().collect(),
            show_hidden: false,
            message: None,
            list_inner: Rect::default(),
            done_button: Rect::default(),
            cancel_button: Rect::default(),
        };
        p.reload(None);
        p
    }

    /// Re-read the folder. The cursor goes to the entry called `focus` if there is one.
    pub fn reload(&mut self, focus: Option<&str>) {
        self.entries.clear();
        self.message = None;
        if self.dir.parent().is_some() {
            self.entries.push(Entry {
                name: "..".into(),
                path: self.dir.parent().unwrap().to_path_buf(),
                kind: Kind::Parent,
                size: 0,
            });
        }
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        match std::fs::read_dir(&self.dir) {
            Ok(read) => {
                for e in read.filter_map(Result::ok) {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if !self.show_hidden && name.starts_with('.') {
                        continue;
                    }
                    let path = e.path();
                    if path.is_dir() {
                        dirs.push(Entry {
                            name,
                            path,
                            kind: Kind::Dir,
                            size: 0,
                        });
                    } else if path.is_file() && is_audio_file_name(&name) {
                        let size = e.metadata().map_or(0, |m| m.len());
                        files.push(Entry {
                            name,
                            path,
                            kind: Kind::Audio,
                            size,
                        });
                    }
                }
            }
            Err(e) => self.message = Some(format!("Cannot read {}: {e}", self.dir.display())),
        }
        dirs.sort_by(|a, b| natural_cmp(&a.name, &b.name));
        files.sort_by(|a, b| natural_cmp(&a.name, &b.name));
        self.entries.extend(dirs);
        self.entries.extend(files);

        let at = focus
            .and_then(|f| self.entries.iter().position(|e| e.name == f))
            .or_else(|| self.entries.iter().position(|e| e.kind != Kind::Parent))
            .or(if self.entries.is_empty() { None } else { Some(0) });
        self.state = ListState::default();
        self.state.select(at);
    }

    pub fn current(&self) -> Option<&Entry> {
        self.state.selected().and_then(|i| self.entries.get(i))
    }

    pub fn move_cursor(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let cur = self.state.selected().unwrap_or(0);
        self.state
            .select(Some(cur.saturating_add_signed(delta).min(self.entries.len() - 1)));
    }

    fn go_to(&mut self, dir: PathBuf, focus: Option<&str>) {
        self.dir = dir;
        self.reload(focus);
    }

    pub fn go_up(&mut self) {
        if let Some(parent) = self.dir.parent().map(Path::to_path_buf) {
            let came_from = self.dir.file_name().map(|n| n.to_string_lossy().into_owned());
            self.go_to(parent, came_from.as_deref());
        }
    }

    /// Enter a folder; leave an audio file alone.
    pub fn open_dir(&mut self) {
        if let Some(e) = self.current().cloned() {
            match e.kind {
                Kind::Parent => self.go_up(),
                Kind::Dir => self.go_to(e.path, None),
                Kind::Audio => {}
            }
        }
    }

    /// Enter a folder, or tick/untick a file.
    pub fn activate(&mut self) {
        if self.current().is_some_and(|e| e.kind == Kind::Audio) {
            self.toggle_current();
        } else {
            self.open_dir();
        }
    }

    pub fn toggle_current(&mut self) {
        if let Some(e) = self.current().filter(|e| e.kind == Kind::Audio) {
            let path = e.path.clone();
            if !self.selected.remove(&path) {
                self.selected.insert(path);
            }
        }
    }

    /// Tick every audio file in this folder, or untick them all if they already are.
    pub fn toggle_all(&mut self) {
        let here: Vec<PathBuf> = self
            .entries
            .iter()
            .filter(|e| e.kind == Kind::Audio)
            .map(|e| e.path.clone())
            .collect();
        if here.iter().all(|p| self.selected.contains(p)) {
            for p in &here {
                self.selected.remove(p);
            }
        } else {
            self.selected.extend(here);
        }
    }

    pub fn is_selected(&self, e: &Entry) -> bool {
        e.kind == Kind::Audio && self.selected.contains(&e.path)
    }

    /// The ticked files, in natural name order.
    pub fn selection(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self.selected.iter().cloned().collect();
        v.sort_by(|a, b| {
            let name = |p: &PathBuf| {
                p.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            };
            natural_cmp(&name(a), &name(b)).then_with(|| a.cmp(b))
        });
        v
    }

    fn finish(&mut self) -> Outcome {
        if self.selected.is_empty() {
            self.message = Some("Tick at least one file first (Space).".into());
            Outcome::Open
        } else {
            Outcome::Done(self.selection())
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Outcome {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return if key.code == KeyCode::Char('c') {
                Outcome::Cancel
            } else {
                Outcome::Open
            };
        }
        self.message = None;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return Outcome::Cancel,
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(-1),
            KeyCode::Char('g') | KeyCode::Home => self.move_cursor(-(self.entries.len() as isize)),
            KeyCode::Char('G') | KeyCode::End => self.move_cursor(self.entries.len() as isize),
            KeyCode::PageDown => self.move_cursor(10),
            KeyCode::PageUp => self.move_cursor(-10),
            KeyCode::Char(' ') => {
                self.toggle_current();
                self.move_cursor(1);
            }
            KeyCode::Enter => self.activate(),
            KeyCode::Char('l') | KeyCode::Right => self.open_dir(),
            KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace => self.go_up(),
            KeyCode::Char('a') => self.toggle_all(),
            KeyCode::Char('.') => {
                self.show_hidden = !self.show_hidden;
                let keep = self.current().map(|e| e.name.clone());
                self.reload(keep.as_deref());
            }
            KeyCode::Char('c') => return self.finish(),
            _ => {}
        }
        Outcome::Open
    }

    pub fn on_mouse(&mut self, m: MouseEvent) -> Outcome {
        let pos = Position::new(m.column, m.row);
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.done_button.contains(pos) {
                    return self.finish();
                }
                if self.cancel_button.contains(pos) {
                    return Outcome::Cancel;
                }
                if self.list_inner.contains(pos) {
                    let idx = self.state.offset() + usize::from(m.row - self.list_inner.y);
                    if idx < self.entries.len() {
                        self.state.select(Some(idx));
                        self.message = None;
                        self.activate();
                    }
                }
            }
            MouseEventKind::ScrollDown if self.list_inner.contains(pos) => self.move_cursor(3),
            MouseEventKind::ScrollUp if self.list_inner.contains(pos) => self.move_cursor(-3),
            _ => {}
        }
        Outcome::Open
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};
    use std::fs;
    use tempfile::TempDir;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn names(p: &FilePicker) -> Vec<&str> {
        p.entries.iter().map(|e| e.name.as_str()).collect()
    }

    /// downloads/{10-b.flac, 2-a.flac, notes.txt, .hidden.flac, craig/{1-z.ogg}, empty/}
    fn tree() -> TempDir {
        let t = TempDir::new().unwrap();
        let d = t.path().join("downloads");
        fs::create_dir_all(d.join("craig")).unwrap();
        fs::create_dir_all(d.join("empty")).unwrap();
        for f in ["10-b.flac", "2-a.flac", "notes.txt", ".hidden.flac", "craig/1-z.ogg"] {
            fs::write(d.join(f), "x").unwrap();
        }
        t
    }

    #[test]
    fn lists_folders_then_audio_in_natural_order_and_hides_the_rest() {
        let t = tree();
        let p = FilePicker::new(&t.path().join("downloads"), &[]);
        assert_eq!(names(&p), ["..", "craig", "empty", "2-a.flac", "10-b.flac"]);
        assert_eq!(p.current().unwrap().name, "craig", "the cursor skips the parent entry");
    }

    #[test]
    fn dot_toggles_hidden_files() {
        let t = tree();
        let mut p = FilePicker::new(&t.path().join("downloads"), &[]);
        p.on_key(key(KeyCode::Char('.')));
        assert!(names(&p).contains(&".hidden.flac"));
        p.on_key(key(KeyCode::Char('.')));
        assert!(!names(&p).contains(&".hidden.flac"));
    }

    #[test]
    fn enter_descends_and_h_returns_to_the_folder_it_came_from() {
        let t = tree();
        let mut p = FilePicker::new(&t.path().join("downloads"), &[]);
        p.on_key(key(KeyCode::Enter)); // into craig
        assert!(p.dir.ends_with("craig"));
        assert_eq!(names(&p), ["..", "1-z.ogg"]);
        p.on_key(key(KeyCode::Char('h')));
        assert!(p.dir.ends_with("downloads"));
        assert_eq!(p.current().unwrap().name, "craig");
    }

    #[test]
    fn space_ticks_and_moves_on_and_the_selection_survives_folders() {
        let t = tree();
        let mut p = FilePicker::new(&t.path().join("downloads"), &[]);
        p.on_key(key(KeyCode::Char('G'))); // 10-b.flac
        p.on_key(key(KeyCode::Char(' ')));
        p.on_key(key(KeyCode::Char('g')));
        p.on_key(key(KeyCode::Char('j'))); // craig
        p.on_key(key(KeyCode::Enter));
        p.on_key(key(KeyCode::Char('j'))); // 1-z.ogg
        p.on_key(key(KeyCode::Char(' ')));
        let sel: Vec<String> = p
            .selection()
            .iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(sel, ["1-z.ogg", "10-b.flac"]);
        assert_eq!(p.on_key(key(KeyCode::Char('c'))), Outcome::Done(p.selection()));
    }

    #[test]
    fn a_toggles_every_file_here_then_none() {
        let t = tree();
        let mut p = FilePicker::new(&t.path().join("downloads"), &[]);
        p.on_key(key(KeyCode::Char('a')));
        assert_eq!(p.selected.len(), 2);
        p.on_key(key(KeyCode::Char('a')));
        assert!(p.selected.is_empty());
    }

    #[test]
    fn done_needs_a_selection_and_escape_cancels() {
        let t = tree();
        let mut p = FilePicker::new(&t.path().join("downloads"), &[]);
        assert_eq!(p.on_key(key(KeyCode::Char('c'))), Outcome::Open);
        assert!(p.message.as_deref().unwrap().contains("Tick at least one file"));
        assert_eq!(p.on_key(key(KeyCode::Esc)), Outcome::Cancel);
    }

    #[test]
    fn preselected_files_start_ticked() {
        let t = tree();
        let f = t.path().join("downloads/2-a.flac");
        let p = FilePicker::new(&t.path().join("downloads"), std::slice::from_ref(&f));
        assert!(p.is_selected(p.entries.iter().find(|e| e.name == "2-a.flac").unwrap()));
        assert_eq!(p.selection(), [f]);
    }

    #[test]
    fn a_missing_start_folder_falls_back_to_its_nearest_parent() {
        let t = tree();
        let p = FilePicker::new(&t.path().join("downloads/gone/deeper"), &[]);
        assert!(p.dir.ends_with("downloads"));
    }

    #[test]
    fn clicking_a_file_ticks_it_and_clicking_done_finishes() {
        let t = tree();
        let mut p = FilePicker::new(&t.path().join("downloads"), &[]);
        p.list_inner = Rect::new(2, 3, 40, 10);
        p.done_button = Rect::new(2, 14, 12, 1);
        p.cancel_button = Rect::new(20, 14, 10, 1);
        let click = |col, row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        };
        // row 3 is "..", 4 craig, 5 empty, 6 2-a.flac
        assert_eq!(p.on_mouse(click(5, 6)), Outcome::Open);
        assert_eq!(p.selected.len(), 1);
        assert!(matches!(p.on_mouse(click(5, 14)), Outcome::Done(v) if v.len() == 1));
        assert_eq!(p.on_mouse(click(21, 14)), Outcome::Cancel);
        p.on_mouse(click(5, 4)); // into craig
        assert!(p.dir.ends_with("craig"));
    }

    #[test]
    fn the_root_has_no_parent_entry() {
        let p = FilePicker::new(Path::new("/"), &[]);
        assert!(p.entries.iter().all(|e| e.kind != Kind::Parent));
    }
}
