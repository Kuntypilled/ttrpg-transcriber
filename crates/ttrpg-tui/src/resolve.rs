//! The form for tracks whose speaker the campaign does not know (PORTING.md section
//! 7.3, "Resolve unknown speakers"): one row per unknown Discord name with a player
//! and a character. This is only the state and the keys; saving goes through
//! `ttrpg_core::aliases`, and drawing is in `ui/modals.rs`.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ttrpg_core::SpeakerInput;

/// Longest text a field takes. A Discord name or a character is far shorter.
const FIELD_MAX: usize = 48;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Col {
    Player,
    Character,
}

#[derive(Debug, Clone)]
pub struct Row {
    /// The `discord_names` key that will be saved.
    pub stem: String,
    /// A file that has it, for the person to recognise.
    pub file: String,
    pub player: String,
    pub character: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Open,
    Cancel,
    Save,
}

pub struct ResolveForm {
    pub slug: String,
    pub rows: Vec<Row>,
    /// The campaign's characters, offered by Ctrl-N and Ctrl-P.
    pub pcs: Vec<String>,
    pub row: usize,
    pub col: Col,
    /// An error from saving, or a nudge. Cleared by the next key.
    pub message: Option<String>,
    discard_armed: bool,
}

impl ResolveForm {
    /// `unknown` is (stem, example file name), one per distinct stem.
    pub fn new(slug: &str, unknown: Vec<(String, String)>, pcs: Vec<String>) -> Self {
        Self {
            slug: slug.to_string(),
            rows: unknown
                .into_iter()
                .map(|(stem, file)| Row {
                    stem,
                    file,
                    player: String::new(),
                    character: String::new(),
                })
                .collect(),
            pcs,
            row: 0,
            col: Col::Player,
            message: None,
            discard_armed: false,
        }
    }

    pub fn field(&self, row: usize, col: Col) -> &str {
        let r = &self.rows[row];
        match col {
            Col::Player => &r.player,
            Col::Character => &r.character,
        }
    }

    fn field_mut(&mut self) -> &mut String {
        let r = &mut self.rows[self.row];
        match self.col {
            Col::Player => &mut r.player,
            Col::Character => &mut r.character,
        }
    }

    pub fn focus(&mut self, row: usize, col: Col) {
        if row < self.rows.len() {
            self.row = row;
            self.col = col;
            self.discard_armed = false;
            self.message = None;
        }
    }

    /// Rows with something typed in them; the ones the save will write.
    pub fn filled(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| !r.player.trim().is_empty() || !r.character.trim().is_empty())
            .count()
    }

    pub fn dirty(&self) -> bool {
        self.rows
            .iter()
            .any(|r| !r.player.is_empty() || !r.character.is_empty())
    }

    /// Every row, for `ttrpg_core::discord_entries` to skip the blank ones.
    pub fn inputs(&self) -> Vec<SpeakerInput> {
        self.rows
            .iter()
            .map(|r| SpeakerInput {
                stem: r.stem.clone(),
                player: r.player.clone(),
                character: r.character.clone(),
            })
            .collect()
    }

    /// Next field in reading order; true when there is none.
    fn advance(&mut self) -> bool {
        match self.col {
            Col::Player => {
                self.col = Col::Character;
                false
            }
            Col::Character if self.row + 1 < self.rows.len() => {
                self.row += 1;
                self.col = Col::Player;
                false
            }
            Col::Character => true,
        }
    }

    fn retreat(&mut self) {
        match self.col {
            Col::Character => self.col = Col::Player,
            Col::Player if self.row > 0 => {
                self.row -= 1;
                self.col = Col::Character;
            }
            Col::Player => {}
        }
    }

    /// Fill the character field with the next (or previous) character of the campaign.
    fn cycle_character(&mut self, forward: bool) {
        if self.pcs.is_empty() {
            self.message = Some("This campaign lists no characters. Type one.".into());
            return;
        }
        self.col = Col::Character;
        let n = self.pcs.len();
        let now = self.pcs.iter().position(|p| *p == self.rows[self.row].character);
        let next = match (now, forward) {
            (Some(i), true) => (i + 1) % n,
            (Some(i), false) => (i + n - 1) % n,
            (None, true) => 0,
            (None, false) => n - 1,
        };
        self.rows[self.row].character = self.pcs[next].clone();
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Outcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let was_armed = std::mem::take(&mut self.discard_armed);
        self.message = None;
        match key.code {
            KeyCode::Esc => {
                if self.dirty() && !was_armed {
                    self.discard_armed = true;
                    self.message = Some("Esc again discards what you typed. Ctrl-S saves it.".into());
                    return Outcome::Open;
                }
                return Outcome::Cancel;
            }
            KeyCode::Char('s') if ctrl => return Outcome::Save,
            KeyCode::Char('n') if ctrl => self.cycle_character(true),
            KeyCode::Char('p') if ctrl => self.cycle_character(false),
            KeyCode::Char('u') if ctrl => self.field_mut().clear(),
            KeyCode::Enter => {
                if self.advance() {
                    return Outcome::Save;
                }
            }
            KeyCode::Tab => {
                self.advance();
            }
            KeyCode::BackTab => self.retreat(),
            KeyCode::Down if self.row + 1 < self.rows.len() => self.row += 1,
            KeyCode::Up if self.row > 0 => self.row -= 1,
            KeyCode::Backspace => {
                self.field_mut().pop();
            }
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) && !c.is_control() => {
                let f = self.field_mut();
                if f.chars().count() < FIELD_MAX {
                    f.push(c);
                }
            }
            _ => {}
        }
        Outcome::Open
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::{KeyEventKind, KeyEventState};

    use super::*;

    fn k(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn press(f: &mut ResolveForm, code: KeyCode) -> Outcome {
        f.on_key(k(code, KeyModifiers::NONE))
    }

    fn ctrl(f: &mut ResolveForm, c: char) -> Outcome {
        f.on_key(k(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn type_text(f: &mut ResolveForm, s: &str) {
        for c in s.chars() {
            press(f, KeyCode::Char(c));
        }
    }

    fn form() -> ResolveForm {
        ResolveForm::new(
            "alpha",
            vec![
                ("newone".into(), "2-newone.flac".into()),
                ("newtwo".into(), "5-newtwo.flac".into()),
            ],
            vec!["Mara_Quill".into(), "Tomas_Ink".into()],
        )
    }

    #[test]
    fn typing_fills_the_focused_field_and_tab_walks_the_grid_in_reading_order() {
        let mut f = form();
        type_text(&mut f, "Pat");
        press(&mut f, KeyCode::Tab);
        type_text(&mut f, "Mara Quill");
        press(&mut f, KeyCode::Tab);
        assert_eq!((f.row, f.col), (1, Col::Player));
        type_text(&mut f, "Lee");
        press(&mut f, KeyCode::BackTab);
        assert_eq!((f.row, f.col), (0, Col::Character), "back goes to the previous field");
        assert_eq!(f.field(0, Col::Player), "Pat");
        assert_eq!(f.field(0, Col::Character), "Mara Quill");
        assert_eq!(f.field(1, Col::Player), "Lee");
        assert_eq!(f.filled(), 2);
        press(&mut f, KeyCode::Backspace);
        assert_eq!(f.field(0, Col::Character), "Mara Quil");
        ctrl(&mut f, 'u');
        assert_eq!(f.field(0, Col::Character), "");
    }

    #[test]
    fn up_and_down_change_rows_and_keep_the_column() {
        let mut f = form();
        press(&mut f, KeyCode::Tab);
        press(&mut f, KeyCode::Down);
        assert_eq!((f.row, f.col), (1, Col::Character));
        press(&mut f, KeyCode::Down);
        assert_eq!(f.row, 1, "no wrapping past the last row");
        press(&mut f, KeyCode::Up);
        press(&mut f, KeyCode::Up);
        assert_eq!(f.row, 0);
    }

    #[test]
    fn enter_moves_on_and_saves_from_the_last_field() {
        let mut f = form();
        for _ in 0..3 {
            assert_eq!(press(&mut f, KeyCode::Enter), Outcome::Open);
        }
        assert_eq!((f.row, f.col), (1, Col::Character));
        assert_eq!(press(&mut f, KeyCode::Enter), Outcome::Save);
        assert_eq!(ctrl(&mut form(), 's'), Outcome::Save, "Ctrl-S saves from anywhere");
    }

    #[test]
    fn ctrl_n_and_ctrl_p_walk_the_campaigns_characters_from_any_field() {
        let mut f = form();
        ctrl(&mut f, 'n');
        assert_eq!((f.field(0, Col::Character), f.col), ("Mara_Quill", Col::Character));
        ctrl(&mut f, 'n');
        assert_eq!(f.field(0, Col::Character), "Tomas_Ink");
        ctrl(&mut f, 'n');
        assert_eq!(f.field(0, Col::Character), "Mara_Quill", "wraps");
        ctrl(&mut f, 'p');
        assert_eq!(f.field(0, Col::Character), "Tomas_Ink");
        // From text that is not a character, p starts at the end.
        let mut g = form();
        press(&mut g, KeyCode::Tab);
        type_text(&mut g, "Someone Else");
        ctrl(&mut g, 'p');
        assert_eq!(g.field(0, Col::Character), "Tomas_Ink");
        // No characters: a message, nothing changed.
        let mut h = ResolveForm::new("a", vec![("x".into(), "1-x.flac".into())], vec![]);
        ctrl(&mut h, 'n');
        assert!(h.message.as_deref().unwrap().contains("no characters"));
        assert_eq!(h.field(0, Col::Character), "");
    }

    #[test]
    fn escape_asks_before_throwing_away_typing_and_not_before_closing_an_empty_form() {
        let mut f = form();
        assert_eq!(press(&mut f, KeyCode::Esc), Outcome::Cancel, "nothing typed");
        type_text(&mut f, "Pat");
        assert_eq!(press(&mut f, KeyCode::Esc), Outcome::Open);
        assert!(f.message.as_deref().unwrap().contains("Esc again"));
        assert_eq!(press(&mut f, KeyCode::Esc), Outcome::Cancel);
        // Any other key in between takes the warning back.
        let mut g = form();
        type_text(&mut g, "Pat");
        press(&mut g, KeyCode::Esc);
        press(&mut g, KeyCode::Char('x'));
        assert_eq!(press(&mut g, KeyCode::Esc), Outcome::Open, "asks again");
    }

    #[test]
    fn controls_and_overlong_text_are_not_typed_in() {
        let mut f = form();
        f.on_key(k(KeyCode::Char('a'), KeyModifiers::ALT));
        f.on_key(k(KeyCode::Char('\u{7}'), KeyModifiers::NONE));
        assert_eq!(f.field(0, Col::Player), "");
        type_text(&mut f, &"x".repeat(100));
        assert_eq!(f.field(0, Col::Player).chars().count(), FIELD_MAX);
        f.on_key(k(KeyCode::Char('Z'), KeyModifiers::SHIFT));
        assert_eq!(f.field(0, Col::Player).chars().count(), FIELD_MAX, "full is full");
        let mut g = form();
        g.on_key(k(KeyCode::Char('Z'), KeyModifiers::SHIFT));
        assert_eq!(g.field(0, Col::Player), "Z", "shifted letters type");
    }

    #[test]
    fn inputs_carry_every_row_so_the_core_can_skip_the_blank_ones() {
        let mut f = form();
        type_text(&mut f, "Pat");
        let inputs = f.inputs();
        assert_eq!(inputs.len(), 2);
        assert_eq!((inputs[0].stem.as_str(), inputs[0].player.as_str()), ("newone", "Pat"));
        assert_eq!(f.filled(), 1);
    }
}
