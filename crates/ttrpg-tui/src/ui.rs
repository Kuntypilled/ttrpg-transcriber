//! Drawing. Colours are the terminal's own ANSI palette (named colours and
//! modifiers only, never RGB), so the TUI follows whatever theme the terminal has.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, Paragraph, Wrap};
use ttrpg_core::Campaign;

use crate::app::App;

const LABEL: Style = Style::new().fg(Color::Cyan);
const DIM: Style = Style::new().add_modifier(Modifier::DIM);
const ERROR: Style = Style::new().fg(Color::Red);
const SELECTED: Style = Style::new().add_modifier(Modifier::REVERSED);

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [header, body, footer] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(6), Constraint::Length(1)]).areas(frame.area());
    draw_header(frame, header, app);

    let [left, right] = Layout::horizontal([Constraint::Percentage(36), Constraint::Percentage(64)]).areas(body);
    draw_campaigns(frame, left, app);
    draw_details(frame, right, app);
    draw_footer(frame, footer, app);

    if app.show_help {
        draw_help(frame);
    }
}

/// `label · system · world` for the selected campaign.
fn subtitle(c: &Campaign) -> String {
    let mut parts = vec![c.label()];
    parts.extend([c.system(), c.world()].into_iter().filter(|s| !s.is_empty()));
    parts.join(" · ")
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let sub = app
        .selected()
        .map_or_else(|| "no campaign selected".to_string(), |(_, c)| subtitle(c));
    let text = vec![
        Line::from(Span::styled(
            "SESSION TRANSCRIBER",
            Style::new().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(sub, DIM)),
    ];
    frame.render_widget(Paragraph::new(text), area);
}

fn draw_campaigns(frame: &mut Frame, area: Rect, app: &mut App) {
    let block = Block::bordered().title(format!(" Campaigns ({}) ", app.campaigns.len()));
    app.list_inner = block.inner(area);

    if app.campaigns.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled("No campaigns found", DIM)).block(block),
            area,
        );
        return;
    }
    let items: Vec<ListItem> = app
        .campaigns
        .iter()
        .map(|(slug, c)| {
            let pcs = c.pcs().join(", ");
            let sub = if pcs.is_empty() {
                slug.clone()
            } else {
                format!("{slug} · {pcs}")
            };
            ListItem::new(vec![Line::from(c.label()), Line::from(Span::styled(sub, DIM))])
        })
        .collect();
    let list = List::new(items).block(block).highlight_style(SELECTED);
    frame.render_stateful_widget(list, area, &mut app.list_state);
}

/// A short value beside its label.
fn kv(label: &str, value: String) -> Vec<Line<'static>> {
    vec![Line::from(vec![
        Span::styled(format!("{label:<13}"), LABEL),
        Span::raw(value),
    ])]
}

/// A path under its label, indented. Paths are long and have no spaces to wrap
/// at, so sharing a line with the label would break them at arbitrary points.
fn path_row(label: &str, value: String) -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(label.to_string(), LABEL)),
        Line::from(format!("  {value}")),
    ]
}

/// The path summary the GUI showed as a tooltip, plus the review_only state.
fn campaign_lines(app: &App, c: &Campaign) -> Vec<Line<'static>> {
    let paths = match c.paths(&app.settings) {
        Ok(p) => p,
        Err(e) => return vec![Line::from(Span::styled(format!("Cannot resolve paths: {e}"), ERROR))],
    };
    let pcs = c.pcs();
    let review = c.review_only().len();
    [
        kv("slug", paths.slug),
        kv(
            "characters",
            if pcs.is_empty() {
                "(none)".to_string()
            } else {
                pcs.join(", ")
            },
        ),
        kv(
            "review_only",
            if review == 0 {
                "none yet".to_string()
            } else {
                format!("{review} entries")
            },
        ),
        path_row("aliases", paths.aliases_file),
        path_row("audio", paths.audio_base),
        path_row("transcripts", paths.transcripts_dir),
        path_row("chunks", paths.chunks_dir),
        path_row("chroma db", paths.db_path),
    ]
    .concat()
}

fn setup_lines(app: &App) -> Vec<Line<'static>> {
    let text = if app.settings.vault_unconfigured() {
        app.settings.unconfigured_message()
    } else {
        match app.settings.setup_hint() {
            Ok(hint) => hint,
            Err(e) => return vec![Line::from(Span::styled(e.to_string(), ERROR))],
        }
    };
    text.lines().map(|l| Line::from(l.to_string())).collect()
}

fn draw_details(frame: &mut Frame, area: Rect, app: &mut App) {
    let block = Block::bordered().title(" Details ");
    app.details_area = block.inner(area);
    let lines = match app.selected() {
        Some((_, c)) => campaign_lines(app, c),
        None => setup_lines(app),
    };
    let para = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((app.detail_scroll, 0));
    frame.render_widget(para, area);
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let keys = "j/k move  r rescan  ? help  q quit  (click a campaign, wheel to scroll)";
    let line = if app.status.is_empty() {
        Line::from(Span::styled(keys, DIM))
    } else {
        Line::from(vec![
            Span::styled(keys, DIM),
            Span::raw("   "),
            Span::raw(app.status.clone()),
        ])
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_help(frame: &mut Frame) {
    let rows = [
        ("j / Down", "next campaign"),
        ("k / Up", "previous campaign"),
        ("g / Home", "first campaign"),
        ("G / End", "last campaign"),
        ("PgDn / PgUp", "scroll the details pane"),
        ("r", "rescan the vaults"),
        ("?", "this help"),
        ("q / Esc", "quit"),
        ("click", "select a campaign"),
        ("wheel", "move or scroll, by pane"),
    ];
    let mut lines: Vec<Line> = rows
        .iter()
        .map(|(k, v)| Line::from(vec![Span::styled(format!("{k:<13}"), LABEL), Span::raw(*v)]))
        .collect();
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("any key closes this", DIM)));

    let area = frame.area();
    let w = 44.min(area.width);
    let h = (lines.len() as u16 + 2).min(area.height);
    let popup = Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h);
    frame.render_widget(Clear, popup);
    let block = Block::bordered().title(" Keys ").title_alignment(Alignment::Left);
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use std::collections::HashMap;
    use std::fs;
    use tempfile::TempDir;
    use ttrpg_core::Settings;

    fn text(buf: &Buffer) -> String {
        let w = buf.area.width as usize;
        buf.content
            .chunks(w)
            .map(|row| {
                row.iter()
                    .map(|c| c.symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn render(app: &mut App, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app)).unwrap();
        text(term.backend().buffer())
    }

    fn vault_app() -> (TempDir, App) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("vault/0.2_campaigns");
        for (name, body) in [
            (
                "alpha",
                r#"{"campaign": "Alpha Run", "system": "daggerheart", "players": {"Mara Quill": "Pat", "Tomas Ink": "Lee"}, "review_only": {"X": ["y"]}}"#,
            ),
            ("beta", r#"{"campaign": "Beta Run", "system": "dnd-5e"}"#),
        ] {
            fs::create_dir_all(root.join(name)).unwrap();
            fs::write(root.join(name).join("canon_aliases.json"), body).unwrap();
        }
        let env = HashMap::from([
            ("HOME".to_string(), tmp.path().to_string_lossy().into_owned()),
            (
                "GM_VAULT".to_string(),
                tmp.path().join("vault").to_string_lossy().into_owned(),
            ),
            (
                "GM_AUDIO_ROOT".to_string(),
                tmp.path().join("audio").to_string_lossy().into_owned(),
            ),
            (
                "GM_CHROMA_ROOT".to_string(),
                tmp.path().join("chroma").to_string_lossy().into_owned(),
            ),
        ]);
        (tmp, App::new(Settings::from_env_map(env, "ttrpg-tui")))
    }

    #[test]
    fn shows_campaigns_and_the_path_summary() {
        let (tmp, mut app) = vault_app();
        let screen = render(&mut app, 120, 24);
        let root = tmp.path().to_string_lossy().into_owned();
        assert!(screen.contains("Campaigns (2)"), "{screen}");
        assert!(screen.contains("Alpha Run"));
        assert!(screen.contains("alpha · Mara_Quill, Tomas_Ink"));
        assert!(screen.contains("Alpha Run · daggerheart"), "header subtitle:\n{screen}");
        assert!(screen.contains(&format!("{root}/vault/0.2_campaigns/alpha/canon_aliases.json")));
        assert!(screen.contains(&format!("{root}/audio/alpha")));
        assert!(screen.contains(&format!("{root}/chroma/alpha")));
        assert!(screen.contains("characters  "));
        assert!(screen.contains("Mara_Quill, Tomas_Ink"));
        assert!(screen.contains("review_only  1 entries"));
    }

    #[test]
    fn details_follow_the_selection() {
        let (_t, mut app) = vault_app();
        app.on_key(KeyEvent {
            code: KeyCode::Char('j'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        let screen = render(&mut app, 120, 24);
        assert!(screen.contains("Beta Run · dnd-5e"));
        assert!(screen.contains("review_only  none yet"));
        assert!(screen.contains("characters   (none)"));
    }

    #[test]
    fn unconfigured_vault_shows_the_setup_instructions() {
        let env = HashMap::from([("HOME".to_string(), "/nonexistent-home".to_string())]);
        let mut app = App::new(Settings::from_env_map(env, "ttrpg-tui"));
        let screen = render(&mut app, 120, 30);
        assert!(screen.contains("No campaigns found"));
        assert!(screen.contains("No vault is configured"), "{screen}");
        assert!(screen.contains("export GM_VAULTS_ROOT=/path/to/vaults"));
        assert!(screen.contains("ttrpg-tui doctor"));
    }

    #[test]
    fn configured_but_empty_vault_names_where_it_looked() {
        let tmp = TempDir::new().unwrap();
        let env = HashMap::from([
            ("HOME".to_string(), tmp.path().to_string_lossy().into_owned()),
            (
                "GM_VAULT".to_string(),
                tmp.path().join("v").to_string_lossy().into_owned(),
            ),
        ]);
        let mut app = App::new(Settings::from_env_map(env, "ttrpg-tui"));
        let screen = render(&mut app, 120, 24);
        assert!(
            screen.contains(&format!(
                "No campaigns found under {}/v/0.2_campaigns",
                tmp.path().display()
            )),
            "{screen}"
        );
    }

    #[test]
    fn records_areas_for_mouse_hit_testing() {
        let (_t, mut app) = vault_app();
        render(&mut app, 100, 24);
        assert!(app.list_inner.width > 0 && app.list_inner.height > 0);
        assert!(app.details_area.x >= app.list_inner.x + app.list_inner.width);
    }

    #[test]
    fn help_overlay_lists_the_keys() {
        let (_t, mut app) = vault_app();
        app.show_help = true;
        let screen = render(&mut app, 100, 24);
        assert!(screen.contains("Keys"));
        assert!(screen.contains("rescan the vaults"));
    }

    #[test]
    fn survives_a_tiny_terminal() {
        let (_t, mut app) = vault_app();
        app.show_help = true;
        render(&mut app, 10, 4);
        render(&mut app, 1, 1);
    }
}
