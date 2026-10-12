//! Application state and input handling. No drawing and no terminal: everything
//! here can be tested by feeding it events.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::ListState;
use ttrpg_core::{Campaign, Settings, list_campaigns};

/// Each campaign takes two terminal rows in the list: the name, then `slug · characters`.
pub const ITEM_HEIGHT: u16 = 2;

/// Lines moved per wheel notch in the details pane.
const WHEEL_STEP: u16 = 3;

pub struct App {
    pub settings: Settings,
    pub campaigns: Vec<(String, Campaign)>,
    pub list_state: ListState,
    pub detail_scroll: u16,
    pub show_help: bool,
    pub status: String,
    pub should_quit: bool,
    /// Something changed since the last draw.
    pub dirty: bool,
    /// Screen areas of the last draw, for mouse hit-testing.
    pub list_inner: Rect,
    pub details_area: Rect,
}

impl App {
    pub fn new(settings: Settings) -> Self {
        let mut app = App {
            settings,
            campaigns: Vec::new(),
            list_state: ListState::default(),
            detail_scroll: 0,
            show_help: false,
            status: String::new(),
            should_quit: false,
            dirty: true,
            list_inner: Rect::default(),
            details_area: Rect::default(),
        };
        app.campaigns = list_campaigns(&app.settings);
        if !app.campaigns.is_empty() {
            app.list_state.select(Some(0));
        }
        app
    }

    pub fn selected(&self) -> Option<&(String, Campaign)> {
        self.list_state.selected().and_then(|i| self.campaigns.get(i))
    }

    /// Rescan the vaults, keeping the selection on the same campaign when it still exists.
    pub fn reload(&mut self) {
        let keep = self.selected().map(|(slug, _)| slug.clone());
        self.campaigns = list_campaigns(&self.settings);
        let idx = keep
            .and_then(|slug| self.campaigns.iter().position(|(s, _)| *s == slug))
            .or(if self.campaigns.is_empty() { None } else { Some(0) });
        self.list_state.select(idx);
        self.detail_scroll = 0;
        self.status = format!("Rescanned: {} campaign(s)", self.campaigns.len());
        self.dirty = true;
    }

    fn select(&mut self, idx: usize) {
        if idx < self.campaigns.len() && self.list_state.selected() != Some(idx) {
            self.list_state.select(Some(idx));
            self.detail_scroll = 0;
        }
        self.dirty = true;
    }

    fn move_selection(&mut self, delta: isize) {
        if self.campaigns.is_empty() {
            return;
        }
        let last = self.campaigns.len() - 1;
        let cur = self.list_state.selected().unwrap_or(0);
        self.select(cur.saturating_add_signed(delta).min(last));
    }

    fn scroll_details(&mut self, delta: i32) {
        let next = i32::from(self.detail_scroll)
            .saturating_add(delta)
            .clamp(0, i32::from(u16::MAX));
        self.detail_scroll = next as u16;
        self.dirty = true;
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        self.dirty = true;
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        if self.show_help {
            // Any key closes the help overlay; `q` still quits.
            self.show_help = false;
            if key.code == KeyCode::Char('q') {
                self.should_quit = true;
            }
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::Char('g') | KeyCode::Home => self.select(0),
            KeyCode::Char('G') | KeyCode::End => self.select(self.campaigns.len().saturating_sub(1)),
            KeyCode::PageDown => self.scroll_details(i32::from(WHEEL_STEP) * 3),
            KeyCode::PageUp => self.scroll_details(-i32::from(WHEEL_STEP) * 3),
            KeyCode::Char('r') => self.reload(),
            _ => self.dirty = false,
        }
    }

    pub fn on_mouse(&mut self, m: MouseEvent) {
        let pos = Position::new(m.column, m.row);
        let in_list = self.list_inner.contains(pos);
        let in_details = self.details_area.contains(pos);
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) if in_list && !self.show_help => {
                let row = usize::from((m.row - self.list_inner.y) / ITEM_HEIGHT);
                self.select(self.list_state.offset() + row);
            }
            MouseEventKind::ScrollDown if in_list => self.move_selection(1),
            MouseEventKind::ScrollUp if in_list => self.move_selection(-1),
            MouseEventKind::ScrollDown if in_details => self.scroll_details(i32::from(WHEEL_STEP)),
            MouseEventKind::ScrollUp if in_details => self.scroll_details(-i32::from(WHEEL_STEP)),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};
    use std::collections::HashMap;
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

    fn click(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn wheel(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// A vault with `names` as campaigns, built from invented data.
    pub fn app_with_campaigns(names: &[&str]) -> (TempDir, App) {
        let tmp = TempDir::new().unwrap();
        for n in names {
            let dir = tmp.path().join("vault/0.2_campaigns").join(n);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("canon_aliases.json"),
                format!(r#"{{"campaign": "{n}", "system": "daggerheart", "players": {{"Mara Quill": "Pat"}}}}"#),
            )
            .unwrap();
        }
        let env = HashMap::from([
            ("HOME".to_string(), tmp.path().to_string_lossy().into_owned()),
            (
                "GM_VAULT".to_string(),
                tmp.path().join("vault").to_string_lossy().into_owned(),
            ),
        ]);
        (tmp, App::new(Settings::from_env_map(env, "ttrpg-tui")))
    }

    #[test]
    fn starts_on_the_first_campaign() {
        let (_t, app) = app_with_campaigns(&["alpha", "beta", "gamma"]);
        assert_eq!(app.selected().unwrap().0, "alpha");
    }

    #[test]
    fn keys_move_and_clamp() {
        let (_t, mut app) = app_with_campaigns(&["alpha", "beta", "gamma"]);
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.selected().unwrap().0, "beta");
        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.selected().unwrap().0, "gamma", "clamped at the end");
        app.on_key(key(KeyCode::Char('g')));
        assert_eq!(app.selected().unwrap().0, "alpha");
        app.on_key(key(KeyCode::Up));
        assert_eq!(app.selected().unwrap().0, "alpha", "clamped at the start");
        app.on_key(key(KeyCode::Char('G')));
        assert_eq!(app.selected().unwrap().0, "gamma");
    }

    #[test]
    fn quit_keys() {
        for k in [key(KeyCode::Char('q')), key(KeyCode::Esc)] {
            let (_t, mut app) = app_with_campaigns(&["alpha"]);
            app.on_key(k);
            assert!(app.should_quit);
        }
        let (_t, mut app) = app_with_campaigns(&["alpha"]);
        let mut ctrl_c = key(KeyCode::Char('c'));
        ctrl_c.modifiers = KeyModifiers::CONTROL;
        app.on_key(ctrl_c);
        assert!(app.should_quit);
    }

    #[test]
    fn help_overlay_swallows_one_key() {
        let (_t, mut app) = app_with_campaigns(&["alpha", "beta"]);
        app.on_key(key(KeyCode::Char('?')));
        assert!(app.show_help);
        app.on_key(key(KeyCode::Char('j')));
        assert!(!app.show_help);
        assert_eq!(
            app.selected().unwrap().0,
            "alpha",
            "the key that closed help did not also move"
        );
    }

    #[test]
    fn click_selects_the_row_under_the_pointer() {
        let (_t, mut app) = app_with_campaigns(&["alpha", "beta", "gamma"]);
        app.list_inner = Rect::new(1, 4, 20, 10);
        // rows 4-5 are alpha, 6-7 beta, 8-9 gamma
        app.on_mouse(click(3, 7));
        assert_eq!(app.selected().unwrap().0, "beta");
        app.on_mouse(click(3, 8));
        assert_eq!(app.selected().unwrap().0, "gamma");
        app.on_mouse(click(3, 12));
        assert_eq!(app.selected().unwrap().0, "gamma", "below the last row does nothing");
        app.on_mouse(click(40, 4));
        assert_eq!(app.selected().unwrap().0, "gamma", "outside the list does nothing");
    }

    #[test]
    fn wheel_moves_selection_over_the_list_and_scrolls_over_details() {
        let (_t, mut app) = app_with_campaigns(&["alpha", "beta"]);
        app.list_inner = Rect::new(1, 4, 20, 10);
        app.details_area = Rect::new(25, 3, 50, 12);
        app.on_mouse(wheel(MouseEventKind::ScrollDown, 3, 5));
        assert_eq!(app.selected().unwrap().0, "beta");
        app.on_mouse(wheel(MouseEventKind::ScrollDown, 30, 5));
        assert_eq!(app.detail_scroll, WHEEL_STEP);
        app.on_mouse(wheel(MouseEventKind::ScrollUp, 30, 5));
        app.on_mouse(wheel(MouseEventKind::ScrollUp, 30, 5));
        assert_eq!(app.detail_scroll, 0, "never scrolls above the top");
    }

    #[test]
    fn selecting_resets_the_details_scroll() {
        let (_t, mut app) = app_with_campaigns(&["alpha", "beta"]);
        app.on_key(key(KeyCode::PageDown));
        assert!(app.detail_scroll > 0);
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.detail_scroll, 0);
    }

    #[test]
    fn reload_keeps_the_selected_campaign_and_sees_new_ones() {
        let (tmp, mut app) = app_with_campaigns(&["beta", "gamma"]);
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.selected().unwrap().0, "gamma");
        let dir = tmp.path().join("vault/0.2_campaigns/alpha");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("canon_aliases.json"), "{}").unwrap();
        app.on_key(key(KeyCode::Char('r')));
        assert_eq!(app.campaigns.len(), 3);
        assert_eq!(
            app.selected().unwrap().0,
            "gamma",
            "still on gamma, though it moved to index 2"
        );
        assert_eq!(app.status, "Rescanned: 3 campaign(s)");
    }

    #[test]
    fn no_campaigns_means_nothing_selected_and_keys_are_harmless() {
        let (_t, mut app) = app_with_campaigns(&[]);
        assert!(app.selected().is_none());
        for c in ['j', 'k', 'g', 'G'] {
            app.on_key(key(KeyCode::Char(c)));
        }
        app.on_mouse(click(3, 5));
        assert!(app.selected().is_none());
    }
}
