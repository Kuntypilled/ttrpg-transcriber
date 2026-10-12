//! Dialogs drawn over the main screen: help, the tool check, the confirm-before-moving
//! step, and the file picker.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, Paragraph, Wrap};
use ttrpg_core::JobPlan;
use ttrpg_core::campaign::session_folder;
use ttrpg_core::preflight::{DepStatus, PreflightReport};

use super::{BOLD, DIM, ERROR, LABEL, OK, SELECTED, WARN};
use crate::app::{App, Modal};
use crate::picker::{FilePicker, Kind};
use crate::text;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let Some(mut modal) = app.modal.take() else { return };
    match &mut modal {
        Modal::Help => help(frame),
        Modal::Setup => setup(frame, app),
        Modal::Confirm(plan) => confirm(frame, app, plan),
        Modal::Picker(picker) => file_picker(frame, picker),
    }
    app.modal = Some(modal);
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let (w, h) = (w.min(area.width), h.min(area.height));
    Rect::new(area.x + (area.width - w) / 2, area.y + (area.height - h) / 2, w, h)
}

/// Clear a centred box, draw its frame, and return the inside.
fn popup(frame: &mut Frame, w: u16, h: u16, title: &str) -> Rect {
    let rect = centered(frame.area(), w, h);
    frame.render_widget(Clear, rect);
    let block = Block::bordered().title(Span::styled(format!(" {title} "), BOLD));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);
    inner
}

fn key_row(key: &str, what: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:<14}"), LABEL),
        Span::raw(what.to_string()),
    ])
}

fn help(frame: &mut Frame) {
    let mut lines = vec![
        Line::from(Span::styled("Moving around", BOLD)),
        key_row("Tab, h, l", "next / previous pane"),
        key_row("j, k, arrows", "move or scroll in the pane that has focus"),
        key_row("g, G", "first / last (in the log: oldest / newest)"),
        key_row("PgUp, PgDn", "page"),
        key_row("[ ]  or 1 2 3", "Paths / Log / Transcript tab"),
        key_row("z", "zoom the right-hand pane to the full screen"),
        Line::default(),
        Line::from(Span::styled("Working", BOLD)),
        key_row("n", "type the session number (+ and - adjust it)"),
        key_row("f", "choose the Craig audio files"),
        key_row("s", "start: move the files in and transcribe"),
        key_row("x", "cancel the running job"),
        key_row("o", "open the session folder"),
        key_row("c", "clear the chosen files"),
        key_row("r", "rescan campaigns and tools"),
        key_row("!", "what is installed and what is missing"),
        key_row("q", "quit (twice if a job is running)"),
        Line::default(),
        Line::from(Span::styled("Mouse", BOLD)),
        key_row("click", "pick a campaign, a tab, a button, the session number"),
        key_row("wheel", "move or scroll the pane under the pointer"),
        Line::default(),
        Line::from(Span::styled("any key closes this", DIM)),
    ];
    let h = lines.len() as u16 + 2;
    let inner = popup(frame, 70, h, "Keys");
    lines.truncate(usize::from(inner.height));
    frame.render_widget(Paragraph::new(lines), inner);
}

fn setup(frame: &mut Frame, app: &App) {
    let report: &PreflightReport = &app.preflight;
    let mut lines: Vec<Line> = Vec::new();
    for item in &report.items {
        match &item.status {
            DepStatus::Found(where_) => lines.push(Line::from(vec![
                Span::styled("  [OK] ", OK),
                Span::styled(format!("{:<16}", item.name), BOLD),
                Span::styled(where_.clone(), DIM),
            ])),
            DepStatus::Missing(help) => {
                let (tag, style) = if item.required {
                    ("[MISSING] ", ERROR)
                } else {
                    ("[optional] ", WARN)
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("  {tag}"), style),
                    Span::styled(item.name.clone(), BOLD),
                ]));
                lines.extend(help.iter().map(|h| Line::from(Span::styled(format!("      {h}"), DIM))));
            }
        }
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        format!(
            "Tool paths come from GM_WHISPER_BIN, GM_WHISPER_MODEL and GM_LEGACY_DIR, or from whisper_bin, \
             whisper_model and legacy_dir in {}.",
            app.settings.config_path()
        ),
        DIM,
    )));
    lines.push(Line::from(Span::styled("any key closes this (r rescans)", DIM)));
    let area = frame.area();
    let w = area.width.saturating_sub(6).clamp(30, 100);
    let h = (lines.len() as u16 + 6).min(area.height.saturating_sub(2));
    let inner = popup(frame, w, h, "Tools");
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn confirm(frame: &mut Frame, app: &mut App, plan: &JobPlan) {
    let moves = plan.moves.len();
    let overwritten = plan.overwritten();
    let unknown = app.unknown_tracks();
    let mut lines = vec![
        Line::default(),
        pair("Campaign", &plan.slug),
        pair(
            "Session",
            &format!("{}  ({})", plan.session, session_folder(plan.session)),
        ),
        pair(
            "Tracks",
            &format!(
                "{} ({moves} to move in, {} already in the folder)",
                plan.track_count(),
                plan.existing.len()
            ),
        ),
        Line::default(),
    ];
    if moves > 0 {
        lines.push(Line::from(vec![
            Span::styled("MOVE", Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(format!(" (not copy) {moves} file(s) into:")),
        ]));
        lines.push(Line::from(format!("  {}", plan.audio_dir)));
        lines.push(Line::from(Span::styled(
            "The originals disappear from where they are now.",
            DIM,
        )));
    } else {
        lines.push(Line::from(format!(
            "Nothing to move. {} file(s) are already in:",
            plan.existing.len()
        )));
        lines.push(Line::from(format!("  {}", plan.audio_dir)));
        lines.push(Line::from(Span::styled(
            "Tracks that were already transcribed are skipped.",
            DIM,
        )));
    }
    if !overwritten.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            format!(
                "⚠ {} file(s) replace ones with the same name already there: {}",
                overwritten.len(),
                overwritten.join(", ")
            ),
            WARN,
        )));
    }
    if unknown > 0 {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            format!("⚠ {unknown} track(s) have no speaker in discord_names; the transcript labels them by file name."),
            WARN,
        )));
    }
    lines.push(Line::default());

    let area = frame.area();
    let w = area.width.saturating_sub(6).clamp(30, 78);
    // Estimate wrapped height generously; the box never exceeds the screen.
    let wrapped: usize = lines
        .iter()
        .map(|l| (l.width() / usize::from(w.saturating_sub(4)).max(1)) + 1)
        .sum();
    let h = (wrapped as u16 + 4).min(area.height.saturating_sub(2));
    let inner = popup(frame, w, h, "Start transcription?");
    let [body, buttons] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);

    let ok_text = if moves > 0 {
        "[Enter] Move and start"
    } else {
        "[Enter] Start"
    };
    let cancel_text = "[Esc] Cancel";
    let ok_w = u16::try_from(text::width(ok_text)).unwrap_or(0);
    let cancel_w = u16::try_from(text::width(cancel_text)).unwrap_or(0);
    let ok_rect = Rect::new(buttons.x + 1, buttons.y, ok_w, 1).intersection(buttons);
    let cancel_rect = Rect::new(ok_rect.x + ok_w + 3, buttons.y, cancel_w, 1).intersection(buttons);
    app.hit.confirm_ok = ok_rect;
    app.hit.confirm_cancel = cancel_rect;
    let line = Line::from(vec![
        Span::raw(" "),
        Span::styled(ok_text, OK.add_modifier(Modifier::BOLD)),
        Span::raw("   "),
        Span::styled(cancel_text, BOLD),
    ]);
    frame.render_widget(Paragraph::new(line), buttons);
}

fn pair(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {label:<10}"), LABEL),
        Span::raw(value.to_string()),
    ])
}

fn file_picker(frame: &mut Frame, p: &mut FilePicker) {
    let area = frame.area();
    let w = area.width.saturating_sub(6).clamp(24, 100);
    let wanted = u16::try_from(p.entries.len())
        .unwrap_or(u16::MAX)
        .saturating_add(6)
        .max(12);
    let h = wanted.min(area.height.saturating_sub(4)).clamp(8, 32);
    let inner = popup(frame, w, h, "Choose audio files");
    if inner.height < 5 || inner.width < 12 {
        return;
    }
    let [head, list, hint, buttons] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let width = usize::from(inner.width);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            text::truncate_start(&p.dir.to_string_lossy(), width),
            LABEL,
        ))),
        head,
    );

    p.list_inner = list;
    let size_w = 9;
    let name_w = width.saturating_sub(4 + 1 + size_w);
    let items: Vec<ListItem> = p
        .entries
        .iter()
        .map(|e| match e.kind {
            Kind::Parent => ListItem::new(Line::from(Span::styled("    .. (parent folder)", DIM))),
            Kind::Dir => ListItem::new(Line::from(vec![
                Span::raw("    "),
                Span::styled(text::truncate(&format!("{}/", e.name), width.saturating_sub(4)), LABEL),
            ])),
            Kind::Audio => {
                let on = p.is_selected(e);
                let (mark, style) = if on { ("[x] ", OK) } else { ("[ ] ", Style::new()) };
                ListItem::new(Line::from(vec![
                    Span::styled(mark, style),
                    Span::styled(text::fit(&e.name, name_w), style),
                    Span::raw(" "),
                    Span::styled(format!("{:>size_w$}", text::human_size(e.size)), DIM),
                ]))
            }
        })
        .collect();
    frame.render_stateful_widget(List::new(items).highlight_style(SELECTED), list, &mut p.state);
    if p.entries.len() <= 1 && p.message.is_none() {
        let empty = Rect::new(list.x + 2, list.y + 1, list.width.saturating_sub(2), 1).intersection(list);
        frame.render_widget(
            Paragraph::new(Span::styled("No folders or audio files here.", DIM)),
            empty,
        );
    }

    let hint_line = match &p.message {
        Some(m) => Line::from(Span::styled(text::truncate(m, width), WARN)),
        None => Line::from(Span::styled(
            text::truncate(
                "Space tick  a all  l/Enter open  h up  . hidden  c done  Esc cancel",
                width,
            ),
            DIM,
        )),
    };
    frame.render_widget(Paragraph::new(hint_line), hint);

    let n = p.selected.len();
    let done_text = format!("[c] Use {n} file(s)");
    let cancel_text = "[Esc] Cancel";
    let done_w = u16::try_from(text::width(&done_text)).unwrap_or(0);
    let cancel_w = u16::try_from(text::width(cancel_text)).unwrap_or(0);
    p.done_button = Rect::new(buttons.x, buttons.y, done_w, 1).intersection(buttons);
    p.cancel_button = Rect::new(buttons.x + done_w + 3, buttons.y, cancel_w, 1).intersection(buttons);
    let done_style = if n > 0 { OK.add_modifier(Modifier::BOLD) } else { DIM };
    let line = Line::from(vec![
        Span::styled(done_text, done_style),
        Span::raw("   "),
        Span::styled(cancel_text, BOLD),
    ]);
    frame.render_widget(Paragraph::new(line), buttons);
}
