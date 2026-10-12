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
use crate::resolve::{Col, ResolveForm};
use crate::text;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let Some(mut modal) = app.modal.take() else { return };
    match &mut modal {
        Modal::Help => help(frame),
        Modal::Setup => setup(frame, app),
        Modal::Confirm(plan) => confirm(frame, app, plan),
        Modal::Picker(picker) => file_picker(frame, picker),
        Modal::Resolve(form) => resolve(frame, app, form),
        Modal::NoReviewOnly => no_review_only(frame, app),
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
        key_row(
            "i",
            "index the session's transcript into the campaign's memory (ChromaDB)",
        ),
        key_row(
            "a",
            "alias report: likely garbled names, written to alias_candidates.md",
        ),
        key_row("o", "open the session folder"),
        key_row("c", "clear the chosen files"),
        key_row("t", "at the confirm step: keep or drop whisper's invented lines"),
        key_row("r", "resolve unknown speakers (add them to discord_names)"),
        key_row("R", "rescan campaigns and tools"),
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
    lines.push(Line::from(Span::styled("any key closes this (R rescans)", DIM)));
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
            format!("⚠ {unknown} track(s) have no speaker in discord_names; the transcript labels them by file name. Esc, then r, adds them."),
            WARN,
        )));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled("  Invented lines", LABEL)));
    if plan.stock.enabled() {
        lines.push(Line::from(format!("  {}.", plan.stock.describe())));
        let mut note = String::from("  What whisper makes up on a silent stretch; a real one lasts a second or two.");
        if let Some(by) = plan.stock.origin_note() {
            note.push_str(&format!(" {by}"));
        }
        lines.push(Line::from(Span::styled(note, DIM)));
    } else {
        lines.push(Line::from(Span::styled(
            "  Off: lines like \"Thank you.\" that whisper makes up on silence stay in the transcript.",
            WARN,
        )));
        if let Some(by) = plan.stock.origin_note() {
            lines.push(Line::from(Span::styled(format!("  {by}"), DIM)));
        }
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
    let stock_text = if plan.stock.enabled() {
        "[t] Keep invented lines"
    } else {
        "[t] Drop invented lines"
    };
    let ok_w = u16::try_from(text::width(ok_text)).unwrap_or(0);
    let cancel_w = u16::try_from(text::width(cancel_text)).unwrap_or(0);
    let stock_w = u16::try_from(text::width(stock_text)).unwrap_or(0);
    let ok_rect = Rect::new(buttons.x + 1, buttons.y, ok_w, 1).intersection(buttons);
    let cancel_rect = Rect::new(ok_rect.x + ok_w + 3, buttons.y, cancel_w, 1).intersection(buttons);
    let stock_rect = Rect::new(cancel_rect.x + cancel_w + 3, buttons.y, stock_w, 1).intersection(buttons);
    app.hit.confirm_ok = ok_rect;
    app.hit.confirm_cancel = cancel_rect;
    app.hit.confirm_stock = stock_rect;
    let line = Line::from(vec![
        Span::raw(" "),
        Span::styled(ok_text, OK.add_modifier(Modifier::BOLD)),
        Span::raw("   "),
        Span::styled(cancel_text, BOLD),
        Span::raw("   "),
        Span::styled(stock_text, LABEL),
    ]);
    frame.render_widget(Paragraph::new(line), buttons);
}

/// Indexing a campaign with no `review_only` list: PORTING.md section 7.3's three choices.
fn no_review_only(frame: &mut Frame, app: &mut App) {
    let lines = vec![
        Line::default(),
        Line::from("  This campaign has no review_only list yet."),
        Line::default(),
        Line::from(
            "  Names that Whisper garbles into real English words (\"borrow\" for a name like Corrow) are \
             never fixed automatically. Indexing now stores them as they are, and fixing them later means \
             indexing again.",
        ),
        Line::default(),
        Line::from(Span::styled(
            "  The alias report reads the transcript and lists the likely ones to copy into canon_aliases.json. \
             It writes nothing to that file.",
            DIM,
        )),
        Line::default(),
    ];
    let area = frame.area();
    let w = area.width.saturating_sub(6).clamp(34, 74);
    let wrapped: usize = lines
        .iter()
        .map(|l| (l.width() / usize::from(w.saturating_sub(4)).max(1)) + 1)
        .sum();
    let h = (wrapped as u16 + 4).min(area.height.saturating_sub(2));
    let inner = popup(frame, w, h, "No review_only list");
    let [body, buttons] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);

    let report_text = "[Enter] Run the report first";
    let index_text = "[i] Index anyway";
    let cancel_text = "[Esc] Cancel";
    let width = |t: &str| u16::try_from(text::width(t)).unwrap_or(0);
    let r = Rect::new(buttons.x + 1, buttons.y, width(report_text), 1).intersection(buttons);
    let i = Rect::new(r.x + width(report_text) + 3, buttons.y, width(index_text), 1).intersection(buttons);
    let c = Rect::new(i.x + width(index_text) + 3, buttons.y, width(cancel_text), 1).intersection(buttons);
    (app.hit.ask_report, app.hit.ask_index, app.hit.ask_cancel) = (r, i, c);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled(report_text, OK.add_modifier(Modifier::BOLD)),
            Span::raw("   "),
            Span::styled(index_text, LABEL),
            Span::raw("   "),
            Span::styled(cancel_text, BOLD),
        ])),
        buttons,
    );
}

/// The resolve-speakers form: one row per unknown Discord name.
fn resolve(frame: &mut Frame, app: &mut App, form: &ResolveForm) {
    app.hit.resolve_fields.clear();
    let area = frame.area();
    let w = area.width.saturating_sub(6).clamp(40, 96);
    let iw = usize::from(w.saturating_sub(2));
    let intro = [
        format!("These tracks are not in {}'s discord_names.", form.slug),
        "Fill in who is speaking on each one, or leave its row empty to skip it.".to_string(),
        "An empty character means the GM.".to_string(),
    ];
    let intro_h: u16 = intro
        .iter()
        .map(|l| u16::try_from(text::width(l).div_ceil(iw.max(1)).max(1)).unwrap_or(1))
        .sum();
    // intro, blank, header, table, key line, characters line, message, buttons
    let fixed = intro_h + 6;
    let visible = usize::from(area.height.saturating_sub(fixed + 4))
        .clamp(1, 12)
        .min(form.rows.len().max(1));
    let h = (fixed + visible as u16 + 2).min(area.height.saturating_sub(2));
    let inner = popup(frame, w, h, "Who is speaking?");
    let [intro_a, _, head_a, table_a, key_a, chars_a, msg_a, btn_a] = Layout::vertical([
        Constraint::Length(intro_h),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(intro.iter().map(|l| Line::from(l.clone())).collect::<Vec<_>>()).wrap(Wrap { trim: false }),
        intro_a,
    );

    // Columns: marker, track, player, character.
    let total = usize::from(inner.width);
    let track_w = (total / 4).clamp(10, 26);
    let rest = total.saturating_sub(2 + track_w + 2);
    let player_w = (rest / 2).max(6);
    let char_w = rest.saturating_sub(player_w + 1).max(6);
    let pad = |s: &str, w: usize| text::fit(s, w);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(
                "  {} {} {}",
                pad("TRACK", track_w),
                pad("PLAYER", player_w),
                pad("CHARACTER", char_w)
            ),
            LABEL,
        ))),
        head_a,
    );

    let rows_visible = usize::from(table_a.height).max(1);
    let first = form
        .row
        .saturating_sub(rows_visible - 1)
        .min(form.rows.len().saturating_sub(rows_visible));
    for (slot, idx) in (first..form.rows.len()).take(rows_visible).enumerate() {
        let y = table_a.y + slot as u16;
        let r = &form.rows[idx];
        let selected = idx == form.row;
        let marker = if selected { "> " } else { "  " };
        let mut spans = vec![
            Span::styled(marker, BOLD),
            Span::raw(pad(&text::truncate(&r.stem, track_w), track_w)),
            Span::raw(" "),
        ];
        let mut x = table_a.x + 2 + track_w as u16 + 1;
        for (col, value, fw) in [
            (Col::Player, &r.player, player_w),
            (Col::Character, &r.character, char_w),
        ] {
            let focused = selected && form.col == col;
            let shown = if focused {
                format!("{}▏", text::truncate_start(value, fw.saturating_sub(1)))
            } else {
                text::truncate_start(value, fw)
            };
            let style = if focused {
                Style::new().add_modifier(Modifier::REVERSED)
            } else {
                Style::new().add_modifier(Modifier::UNDERLINED)
            };
            spans.push(Span::styled(pad(&shown, fw), style));
            spans.push(Span::raw(" "));
            app.hit
                .resolve_fields
                .push((Rect::new(x, y, fw as u16, 1).intersection(table_a), idx, col));
            x += fw as u16 + 1;
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(table_a.x, y, table_a.width, 1),
        );
    }

    if let Some(r) = form.rows.get(form.row) {
        let more = if form.rows.len() > rows_visible {
            format!("   (row {} of {})", form.row + 1, form.rows.len())
        } else {
            String::new()
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                text::truncate(
                    &format!("Saved as discord_names key \"{}\", from {}{more}", r.stem, r.file),
                    total,
                ),
                DIM,
            ))),
            key_a,
        );
    }
    let chars = if form.pcs.is_empty() {
        "This campaign lists no characters; type one.".to_string()
    } else {
        format!("Characters: {}", form.pcs.join(", "))
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(text::truncate(&chars, total), DIM))),
        chars_a,
    );
    if let Some(m) = &form.message {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(text::truncate(m, total), WARN))),
            msg_a,
        );
    }

    let save_text = "[Ctrl-S] Save to campaign";
    let cancel_text = "[Esc] Cancel";
    let save_w = u16::try_from(text::width(save_text)).unwrap_or(0);
    let cancel_w = u16::try_from(text::width(cancel_text)).unwrap_or(0);
    let save_rect = Rect::new(btn_a.x + 1, btn_a.y, save_w, 1).intersection(btn_a);
    let cancel_rect = Rect::new(save_rect.x + save_w + 3, btn_a.y, cancel_w, 1).intersection(btn_a);
    app.hit.resolve_save = save_rect;
    app.hit.resolve_cancel = cancel_rect;
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled(save_text, OK.add_modifier(Modifier::BOLD)),
            Span::raw("   "),
            Span::styled(cancel_text, BOLD),
            Span::raw("   "),
            Span::styled("Tab next field   Ctrl-N / Ctrl-P pick a character", DIM),
        ])),
        btn_a,
    );
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
