//! Drawing. Colours are the terminal's own ANSI palette (named colours and
//! modifiers only, never RGB), so the TUI follows whatever theme the terminal has.
//!
//! Layout:
//!
//! ```text
//! title / campaign line                                   (banner when tools are missing)
//! +- Campaigns -+ +- Session ------------------+ +- Paths | Log | Transcript -+
//! |             | | Session [17]  -> S17       | |                            |
//! |             | | Files (5)  all resolved    | |                            |
//! +-------------+ +----------------------------+ +----------------------------+
//! progress bar
//! status line
//! [f] Files  [s] Start  ...                                     (clickable)
//! ```

mod modals;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Gauge, List, ListItem, Paragraph, Wrap};
use ttrpg_core::Campaign;
use ttrpg_core::audio::Speaker;
use ttrpg_core::is_whisper_segment_line;

use crate::app::{Action, App, Focus, Hit, StatusKind, Tab};
use crate::text;

const LABEL: Style = Style::new().fg(Color::Cyan);
const DIM: Style = Style::new().add_modifier(Modifier::DIM);
const BOLD: Style = Style::new().add_modifier(Modifier::BOLD);
const ERROR: Style = Style::new().fg(Color::Red);
const WARN: Style = Style::new().fg(Color::Yellow);
const OK: Style = Style::new().fg(Color::Green);
const SELECTED: Style = Style::new().add_modifier(Modifier::REVERSED);

pub fn draw(frame: &mut Frame, app: &mut App) {
    app.hit = Hit::default();
    let [header, body, gauge, status, buttons] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(5),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    if app.zoom {
        draw_header(frame, header, app);
        draw_preview(frame, body, app);
        draw_gauge(frame, gauge, app);
        draw_status(frame, status, app);
        draw_buttons(frame, buttons, app);
        modals::draw(frame, app);
        return;
    }
    let [left, middle, right] = Layout::horizontal([
        Constraint::Percentage(20),
        Constraint::Percentage(34),
        Constraint::Percentage(46),
    ])
    .areas(body);

    draw_header(frame, header, app);
    draw_campaigns(frame, left, app);
    draw_session(frame, middle, app);
    draw_preview(frame, right, app);
    draw_gauge(frame, gauge, app);
    draw_status(frame, status, app);
    draw_buttons(frame, buttons, app);
    modals::draw(frame, app);
}

/// A bordered pane whose frame is brighter when it has the keyboard.
fn pane(title: &str, focused: bool) -> Block<'static> {
    let border = if focused {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new()
    };
    let title_style = if focused {
        border.add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    };
    Block::bordered()
        .border_style(border)
        .title(Span::styled(format!(" {title} "), title_style))
}

fn cursor_style(focused: bool) -> Style {
    if focused { SELECTED } else { BOLD }
}

/// `label · system · world` for a campaign.
fn subtitle(c: &Campaign) -> String {
    let mut parts = vec![c.label()];
    parts.extend([c.system(), c.world()].into_iter().filter(|s| !s.is_empty()));
    parts.join(" · ")
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let sub = app
        .selected()
        .map_or_else(|| "no campaign selected".to_string(), |(_, c)| subtitle(c));
    let mut second = vec![Span::styled(sub, DIM)];
    if !app.preflight.ok() {
        second.push(Span::raw("   "));
        second.push(Span::styled(
            format!(
                "! cannot transcribe yet: missing {}  (press !)",
                app.preflight.missing_names()
            ),
            ERROR,
        ));
    }
    let lines = vec![
        Line::from(Span::styled("SESSION TRANSCRIBER", BOLD)),
        Line::from(second),
    ];
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_campaigns(frame: &mut Frame, area: Rect, app: &mut App) {
    let focused = app.focus == Focus::Campaigns;
    let block = pane(&format!("Campaigns ({})", app.campaigns.len()), focused);
    app.hit.campaigns = block.inner(area);
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
    let list = List::new(items).block(block).highlight_style(cursor_style(focused));
    frame.render_stateful_widget(list, area, &mut app.list_state);
}

// -- session pane ------------------------------------------------------------------------

fn speaker_text(s: &Speaker) -> (String, Style) {
    match s {
        Speaker::Character { character, player } => (
            match player {
                Some(p) => format!("{character} ({p})"),
                None => character.clone(),
            },
            Style::new(),
        ),
        Speaker::Gm { player } => (
            match player {
                Some(p) => format!("GM ({p})"),
                None => "GM".to_string(),
            },
            LABEL,
        ),
        Speaker::Unknown { label, .. } => (format!("⚠ unknown (\"{label}\")"), WARN),
    }
}

fn draw_session(frame: &mut Frame, area: Rect, app: &mut App) {
    let focused = app.focus == Focus::Session;
    let block = pane("Session", focused);
    let inner = block.inner(area);
    app.hit.session_area = area;
    frame.render_widget(block, area);
    if inner.width < 4 || inner.height < 2 {
        return;
    }
    let [info, files] = Layout::vertical([Constraint::Length(5), Constraint::Min(0)]).areas(inner);
    let w = usize::from(inner.width);

    // Session number line.
    let mut lines: Vec<Line> = Vec::new();
    let label_w = 11;
    match (app.session, &app.editing) {
        (None, _) => lines.push(Line::from(Span::styled("Pick a campaign to begin.", DIM))),
        (Some(n), editing) => {
            let field = match editing {
                Some(buf) => format!("[{buf}_ ]"),
                None => format!("[ {n} ]"),
            };
            let hint = match editing {
                Some(_) => "  Enter to accept".to_string(),
                None => format!("  → S{n:02}   n edit"),
            };
            app.hit.session_field = Rect::new(
                info.x + label_w,
                info.y,
                u16::try_from(text::width(&field))
                    .unwrap_or(0)
                    .min(info.width.saturating_sub(label_w)),
                1,
            );
            let field_style = if editing.is_some() { SELECTED } else { BOLD };
            lines.push(Line::from(vec![
                Span::styled(format!("{:<width$}", "Session", width = usize::from(label_w)), LABEL),
                Span::styled(field, field_style),
                Span::styled(text::truncate(&hint, w.saturating_sub(usize::from(label_w) + 8)), DIM),
            ]));
        }
    }
    if let Some(sp) = app.session_paths() {
        let folder_exists = std::path::Path::new(&sp.audio_dir).is_dir();
        let suffix = if folder_exists { "" } else { "  (new)" };
        lines.push(kv_line("Audio dir", &format!("{}{suffix}", sp.audio_dir), w, label_w));
        let (state, style) = if std::path::Path::new(&sp.transcript_md).is_file() {
            ("transcript.md exists ✓", OK)
        } else {
            ("no transcript yet", DIM)
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{:<width$}", "Result", width = usize::from(label_w)), LABEL),
            Span::styled(state, style),
        ]));
    }
    lines.push(review_banner(app, w));
    let unknown = app.unknown_tracks();
    let summary = if app.tracks.is_empty() {
        Span::styled("none yet", DIM)
    } else if unknown > 0 {
        Span::styled(format!("⚠ {unknown} unknown speaker(s)"), WARN)
    } else {
        Span::styled("all speakers resolved ✓", OK)
    };
    lines.push(Line::from(vec![
        Span::styled(format!("Files ({})", app.tracks.len()), BOLD),
        Span::raw("   "),
        summary,
    ]));
    frame.render_widget(Paragraph::new(lines), info);

    // Track list.
    app.hit.tracks = files;
    if app.tracks.is_empty() {
        let hint = if app.campaigns.is_empty() {
            vec![]
        } else {
            vec![
                Line::from(Span::styled("No audio yet.", DIM)),
                Line::from(vec![
                    Span::styled("Press ", DIM),
                    Span::styled("f", LABEL),
                    Span::styled(" to choose the Craig files.", DIM),
                ]),
            ]
        };
        frame.render_widget(Paragraph::new(hint), files);
        return;
    }
    let longest = app.tracks.iter().map(|t| text::width(&t.name)).max().unwrap_or(10);
    // One line per track when the name and a readable speaker both fit; otherwise the
    // speaker goes on a line of its own under the full file name.
    let one_line = w >= 2 + longest + 2 + 26;
    app.hit.track_row_height = if one_line { 1 } else { 2 };
    let app_report = app.report.as_ref();
    let items: Vec<ListItem> = app
        .tracks
        .iter()
        .map(|t| {
            let (marker, marker_style) = match (outcome(app_report, &t.name), t.incoming, t.in_folder) {
                (Some(true), ..) => ("✓ ", OK),
                (Some(false), ..) => ("✗ ", ERROR),
                (None, true, false) => ("+ ", OK),
                (None, true, true) => ("↻ ", WARN),
                (None, false, _) => ("· ", DIM),
            };
            let (who, style) = speaker_text(&t.speaker);
            if one_line {
                let room = w.saturating_sub(2 + longest + 2);
                ListItem::new(Line::from(vec![
                    Span::styled(marker, marker_style),
                    Span::raw(text::fit(&t.name, longest)),
                    Span::raw("  "),
                    Span::styled(text::truncate(&format!("→ {who}"), room), style),
                ]))
            } else {
                ListItem::new(vec![
                    Line::from(vec![
                        Span::styled(marker, marker_style),
                        Span::raw(text::truncate(&t.name, w.saturating_sub(2))),
                    ]),
                    Line::from(vec![
                        Span::raw("    "),
                        Span::styled(text::truncate(&format!("→ {who}"), w.saturating_sub(4)), style),
                    ]),
                ])
            }
        })
        .collect();
    let list = List::new(items).highlight_style(cursor_style(focused));
    frame.render_stateful_widget(list, files, &mut app.track_state);
}

/// How the last run treated a track: `Some(true)` done (now or before), `Some(false)` failed.
fn outcome(report: Option<&ttrpg_core::RunReport>, name: &str) -> Option<bool> {
    let r = report?;
    if r.failed.iter().any(|(f, _)| f == name) {
        Some(false)
    } else if r.succeeded.iter().chain(&r.skipped).any(|f| f == name) {
        Some(true)
    } else {
        None
    }
}

fn kv_line(label: &str, value: &str, width: usize, label_w: u16) -> Line<'static> {
    let room = width.saturating_sub(usize::from(label_w));
    Line::from(vec![
        Span::styled(format!("{label:<width$}", width = usize::from(label_w)), LABEL),
        Span::styled(text::truncate_start(value, room), DIM),
    ])
}

// -- preview pane ------------------------------------------------------------------------

fn draw_preview(frame: &mut Frame, area: Rect, app: &mut App) {
    let focused = app.focus == Focus::Preview;
    let border = if focused {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new()
    };
    // The tab names are the pane's title; each is a click target.
    let mut spans = vec![Span::raw(" ")];
    let mut x = area.x.saturating_add(2);
    for tab in Tab::ALL {
        let label = format!(" {} ", tab.title());
        let w = u16::try_from(text::width(&label)).unwrap_or(0);
        app.hit.tabs.push((Rect::new(x, area.y, w, 1).intersection(area), tab));
        let style = if tab == app.tab {
            if focused {
                SELECTED
            } else {
                SELECTED.add_modifier(Modifier::DIM)
            }
        } else {
            Style::new()
        };
        spans.push(Span::styled(label, style));
        x = x.saturating_add(w);
    }
    let mut block = Block::bordered().border_style(border).title(Line::from(spans));
    if app.zoom {
        block =
            block.title_bottom(Line::from(Span::styled(" z or Esc restores the other panes ", DIM)).right_aligned());
    }
    let inner = block.inner(area);
    app.hit.preview_area = area;
    app.hit.preview_inner = inner;

    match app.tab {
        Tab::Paths => {
            let lines = paths_lines(app);
            app.paths_scroll = app.paths_scroll.min(lines.len().saturating_sub(1));
            let scroll = u16::try_from(app.paths_scroll).unwrap_or(u16::MAX);
            frame.render_widget(
                Paragraph::new(lines)
                    .block(block)
                    .wrap(Wrap { trim: false })
                    .scroll((scroll, 0)),
                area,
            );
        }
        Tab::Log => {
            let (lines, back) = log_view(app, inner);
            if back > 0 {
                block = block.title_bottom(Line::from(Span::styled(
                    format!(" scrolled back {back} lines, G follows "),
                    WARN,
                )));
            }
            frame.render_widget(Paragraph::new(lines).block(block), area);
        }
        Tab::Transcript => {
            let (lines, note) = transcript_view(app, inner);
            if let Some(note) = note {
                block = block.title_bottom(Line::from(Span::styled(note, DIM)));
            }
            frame.render_widget(Paragraph::new(lines).block(block), area);
        }
    }
}

fn kv(label: &str, value: String) -> Line<'static> {
    Line::from(vec![Span::styled(format!("{label:<13}"), LABEL), Span::raw(value)])
}

fn path_row(label: &str, value: String) -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(label.to_string(), LABEL)),
        Line::from(format!("  {value}")),
    ]
}

/// Campaign paths, then the session's paths when a session is set.
fn paths_lines(app: &App) -> Vec<Line<'static>> {
    let Some(c) = app.campaign() else {
        return setup_lines(app);
    };
    let paths = match c.paths(&app.settings) {
        Ok(p) => p,
        Err(e) => return vec![Line::from(Span::styled(format!("Cannot resolve paths: {e}"), ERROR))],
    };
    let pcs = c.pcs();
    let review = c.review_only().len();
    let mut out = Vec::new();
    out.extend([
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
    ]);
    out.extend(path_row("aliases", paths.aliases_file));
    out.extend(path_row("chroma db", paths.db_path));
    if let Some(sp) = app.session_paths() {
        out.push(Line::default());
        out.push(Line::from(Span::styled(format!("Session {}", sp.session), BOLD)));
        out.extend(path_row("audio", sp.audio_dir));
        out.extend(path_row("transcripts", sp.session_dir));
        out.extend(path_row("transcript.md", sp.transcript_md));
        out.extend(path_row("chunks", sp.chunks_file));
        out.extend(path_row("work (temporary)", sp.work_dir));
    }
    out
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

/// The newest lines that fit, honouring how far the user has scrolled back.
/// Returns the lines and the effective scrolled-back amount.
fn log_view(app: &mut App, inner: Rect) -> (Vec<Line<'static>>, usize) {
    if app.log.is_empty() {
        let hint = if app.running() {
            "Waiting for the script's first line..."
        } else {
            "Nothing yet. Start a run (s) and the script's output appears here."
        };
        return (vec![Line::from(Span::styled(hint, DIM))], 0);
    }
    // Style each source line once, then wrap it, so a long line keeps its style on
    // every row it wraps to. Whisper's own segment lines are raw output (they still
    // contain the invented "Thank you." lines the script cleans up afterwards), so
    // they are dimmed to let the script's lines stand out.
    let width = usize::from(inner.width);
    let mut wrapped: Vec<(String, Style)> = Vec::new();
    for line in &app.log {
        let style = if is_whisper_segment_line(line) {
            DIM
        } else {
            log_style(line)
        };
        wrapped.extend(text::wrap([line.as_str()], width).into_iter().map(|l| (l, style)));
    }
    let height = usize::from(inner.height);
    app.log_back = app.log_back.min(wrapped.len().saturating_sub(height));
    let end = wrapped.len() - app.log_back;
    let start = end.saturating_sub(height);
    let lines = wrapped[start..end]
        .iter()
        .map(|(l, style)| Line::from(Span::styled(l.clone(), *style)))
        .collect();
    (lines, app.log_back)
}

fn log_style(line: &str) -> Style {
    let t = line.trim_start();
    if t.starts_with("== ") {
        BOLD
    } else if t.starts_with("ERROR") || t.starts_with("failed:") || t.contains("[MISSING]") {
        ERROR
    } else if t.starts_with("WARNING") || t.starts_with("Cancelling") {
        WARN
    } else if t.contains("[OK]") || t.starts_with("Done:") {
        OK
    } else {
        Style::new()
    }
}

fn transcript_view(app: &mut App, inner: Rect) -> (Vec<Line<'static>>, Option<String>) {
    let Some(t) = &app.transcript else {
        let msg = match app.session {
            Some(n) => format!("No transcript.md for session {n} yet."),
            None => "No session selected.".to_string(),
        };
        return (vec![Line::from(Span::styled(msg, DIM))], None);
    };
    let w = usize::from(inner.width);
    // transcript.md alternates `**[hh:mm:ss] Speaker:**` header lines with text.
    let mut rows: Vec<(String, bool)> = Vec::new();
    for line in &t.lines {
        let header = line
            .trim_end()
            .strip_prefix("**[")
            .and_then(|r| r.strip_suffix(":**"))
            .and_then(|r| r.split_once("] "));
        match header {
            Some((time, who)) => rows.push((format!("[{time}] {who}"), true)),
            None => rows.extend(text::wrap([line.as_str()], w).into_iter().map(|l| (l, false))),
        }
    }
    let height = usize::from(inner.height);
    app.transcript_scroll = app.transcript_scroll.min(rows.len().saturating_sub(height));
    let start = app.transcript_scroll;
    let end = (start + height).min(rows.len());
    let lines = rows[start..end]
        .iter()
        .map(|(l, header)| {
            if *header {
                Line::from(Span::styled(text::truncate(l, w), LABEL.add_modifier(Modifier::BOLD)))
            } else {
                Line::from(l.clone())
            }
        })
        .collect();
    (lines, Some(format!(" lines {}-{} of {} ", start + 1, end, rows.len())))
}

/// The standing nudge for a campaign with no `review_only` list: the alias report seeds it
/// before the first index.
fn review_banner(app: &App, width: usize) -> Line<'static> {
    let Some(camp) = app.campaign() else {
        return Line::default();
    };
    if camp.has_review_only() {
        return Line::default();
    }
    let transcribed = camp.session_numbers(&app.settings).is_ok_and(|n| !n.is_empty());
    if transcribed {
        Line::from(Span::styled(
            text::truncate("⚠ no review_only list: a = alias report", width),
            WARN,
        ))
    } else {
        Line::from(Span::styled(
            text::truncate("no review_only yet (a = alias report)", width),
            DIM,
        ))
    }
}

// -- bottom rows ----------------------------------------------------------------------------

const SPINNER: [char; 4] = ['|', '/', '-', '\\'];

fn draw_gauge(frame: &mut Frame, area: Rect, app: &App) {
    let t = &app.tracker;
    let (ratio, label, color) = if let Some(job) = &app.job {
        let spin = SPINNER[(app.tick / 3) as usize % SPINNER.len()];
        (
            t.fraction(),
            format!("{spin} {}   {}", t.label(), text::clock(job.started.elapsed())),
            Color::Green,
        )
    } else if let Some(r) = &app.index_report {
        let took = text::clock(r.elapsed);
        if r.ok() {
            (
                1.0,
                format!(
                    "{}   {took}",
                    if r.kind == ttrpg_core::IndexKind::Index {
                        "Indexed"
                    } else {
                        "Report ready"
                    }
                ),
                Color::Green,
            )
        } else if r.cancelled {
            (t.fraction(), format!("Cancelled   {took}"), Color::Yellow)
        } else {
            (t.fraction().max(0.02), format!("Failed   {took}"), Color::Red)
        }
    } else if let Some(r) = &app.report {
        let took = text::clock(r.elapsed);
        if r.ok() {
            (1.0, format!("Complete   {took}"), Color::Green)
        } else if r.cancelled {
            (t.fraction(), format!("Cancelled   {took}"), Color::Yellow)
        } else {
            (
                t.fraction().max(0.02),
                format!("Finished with problems   {took}"),
                Color::Red,
            )
        }
    } else {
        (0.0, "Ready".to_string(), Color::Reset)
    };
    let gauge = Gauge::default()
        .ratio(ratio.clamp(0.0, 1.0))
        .label(Span::styled(label, BOLD))
        .gauge_style(Style::new().fg(color))
        .use_unicode(true);
    frame.render_widget(gauge, area);
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let style = match app.status.kind {
        StatusKind::Info => Style::new(),
        StatusKind::Ok => OK,
        StatusKind::Warn => WARN,
        StatusKind::Error => ERROR,
    };
    let line = Line::from(Span::styled(
        text::truncate(&app.status.text, usize::from(area.width)),
        style,
    ));
    frame.render_widget(Paragraph::new(line), area);
}

/// The clickable button bar. Disabled buttons stay visible and say why when used. When
/// the terminal is too narrow for all of them the least used drop out first (their keys
/// still work), so Help and Quit are always there.
fn draw_buttons(frame: &mut Frame, area: Rect, app: &mut App) {
    // (key, label, action, drop order: 0 = never dropped, higher goes first)
    let all: [(char, &str, Action, u8); 11] = [
        ('f', "Files", Action::ChooseFiles, 0),
        ('r', "Resolve", Action::Resolve, 3),
        ('s', "Start", Action::Start, 0),
        ('x', "Cancel", Action::Cancel, 0),
        ('i', "Index", Action::Index, 2),
        ('p', "Rebuild", Action::Reprocess, 4),
        ('a', "Report", Action::Report, 5),
        ('o', "Open folder", Action::OpenFolder, 6),
        ('c', "Clear files", Action::ClearFiles, 7),
        ('?', "Help", Action::Help, 0),
        ('q', "Quit", Action::Quit, 0),
    ];
    let width_of = |(key, name, ..): &(char, &str, Action, u8)| text::width(&format!("[{key}] {name}")) + 2;
    let mut shown: Vec<_> = all.to_vec();
    while shown.iter().map(width_of).sum::<usize>().saturating_sub(2) > usize::from(area.width) {
        let Some(idx) = shown
            .iter()
            .enumerate()
            .filter(|(_, b)| b.3 > 0)
            .max_by_key(|(_, b)| b.3)
            .map(|(i, _)| i)
        else {
            break;
        };
        shown.remove(idx);
    }
    let mut spans = Vec::new();
    let mut x = area.x;
    for (key, name, action, _) in shown {
        let text = format!("[{key}] {name}");
        let w = u16::try_from(text::width(&text)).unwrap_or(0);
        if x.saturating_add(w) > area.x.saturating_add(area.width) {
            break;
        }
        let on = app.enabled(action);
        let key_style = if on { LABEL.add_modifier(Modifier::BOLD) } else { DIM };
        let name_style = if on { Style::new() } else { DIM };
        spans.push(Span::styled(format!("[{key}]"), key_style));
        spans.push(Span::styled(format!(" {name}"), name_style));
        spans.push(Span::raw("  "));
        app.hit.buttons.push((Rect::new(x, area.y, w, 1), action));
        x = x.saturating_add(w + 2);
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests;
