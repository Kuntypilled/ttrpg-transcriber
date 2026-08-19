#!/usr/bin/env python3
"""
Session Transcriber: TTRPG session audio transcription GUI (GTK3)

Campaign-agnostic. The dropdown at the top is populated by globbing
$VAULT/0.2_campaigns/*/canon_aliases.json, so adding a campaign means creating
one folder with one JSON file. Selecting a campaign drives the alias file, the
audio folder, the transcript folder, the chunk folder and the Chroma database.

GTK3 bindings are a system package, not a pip install. See INSTALL.md.

Every file in this repo lives in one directory and finds its siblings from
__file__, so the checkout can sit anywhere. Make the entry points executable:
    chmod +x transcribe_gui.py transcribe.sh launch_transcriber.sh

Startup errors are logged to $XDG_STATE_HOME/ttrpg-transcriber/transcribe_gui.log
(override with $GM_TRANSCRIBER_LOG).
"""

import sys
import os
import re
import datetime
import traceback

# Log file for startup and runtime errors (useful when launched from a .desktop
# entry, where stderr goes nowhere). Resolved here rather than imported from
# campaign.py because this has to work even when that import is what failed.
def _default_log_path():
    override = os.environ.get("GM_TRANSCRIBER_LOG")
    if override:
        return os.path.abspath(os.path.expanduser(override))
    state = os.environ.get("XDG_STATE_HOME")
    if not (state and os.path.isabs(state)):
        state = os.path.expanduser("~/.local/state")
    return os.path.join(state, "ttrpg-transcriber", "transcribe_gui.log")


LOG_PATH = _default_log_path()

def _write_startup_log(msg: str):
    try:
        os.makedirs(os.path.dirname(LOG_PATH), exist_ok=True)
        with open(LOG_PATH, "a") as f:
            import datetime
            f.write(f"\n[{datetime.datetime.now().isoformat(timespec='seconds')}] {msg}\n")
    except Exception:
        pass

try:
    import gi
    gi.require_version('Gtk', '3.0')
    from gi.repository import Gtk, GLib, Gdk, Pango
    import shutil
    import subprocess
    import threading
except Exception as e:
    _write_startup_log(f"IMPORT ERROR: {e}\n{traceback.format_exc()}")
    sys.exit(1)

_write_startup_log("Startup OK")

# ── Paths ─────────────────────────────────────────────────────────────────────
# These three are tool locations, not campaign data, so they stay fixed.
# Everything campaign-scoped comes from campaign.py and the selected campaign.

SCRIPT_DIR     = os.path.dirname(os.path.abspath(__file__))
SCRIPT_PATH    = os.path.join(SCRIPT_DIR, "transcribe.sh")
CHUNKS_SCRIPT  = os.path.join(SCRIPT_DIR, "transcript_to_chunks.py")
CHROMA_SCRIPT  = os.path.join(SCRIPT_DIR, "chroma_memory.py")

sys.path.insert(0, SCRIPT_DIR)
try:
    import campaign as campaign_mod
except Exception as e:  # noqa: BLE001 - startup diagnostics go to the log file
    _write_startup_log(f"CAMPAIGN MODULE IMPORT ERROR: {e}\n{traceback.format_exc()}")
    raise

# ── Gruvbox Material Soft Dark ────────────────────────────────────────────────

CSS = """
* {
    -gtk-outline-radius: 4px;
}

window {
    background-color: #32302f;
    color: #d4be98;
}

label {
    color: #d4be98;
}

.title-label {
    color: #d8a657;
    font-size: 18px;
    font-weight: bold;
    letter-spacing: 3px;
}

.subtitle-label {
    color: #928374;
    font-size: 11px;
}

.section-label {
    color: #a9b665;
    font-size: 10px;
    font-weight: bold;
    letter-spacing: 1px;
}

.hint-label {
    color: #665c54;
    font-size: 11px;
    font-style: italic;
}

.warn-label {
    color: #e78a4e;
    font-size: 11px;
}

.preview-label {
    color: #d8a657;
    font-size: 12px;
}

.status-label {
    color: #928374;
    font-size: 11px;
    font-style: italic;
}

/* ── Entry ── */
entry {
    background-image: none;
    background-color: #45403d;
    color: #d4be98;
    border: 1px solid #665c54;
    border-radius: 4px;
    padding: 6px 10px;
    caret-color: #d8a657;
    min-height: 24px;
}

entry:focus {
    border-color: #d8a657;
    box-shadow: none;
    outline: none;
}

/* ── Campaign selector ── */
/* A MenuButton + Popover, not a ComboBox. GtkComboBox pops a GtkMenu, and
   GtkMenuShell decides between click-to-open and drag-to-select from event
   timestamps; under XWayland that heuristic misfires and every button release
   dismisses the menu. A popover just toggles. */
.campaign-btn {
    background-image: none;
    background-color: #45403d;
    color: #d4be98;
    border: 1px solid #665c54;
    border-radius: 4px;
    padding: 6px 10px;
    min-height: 26px;
}

.campaign-btn:hover {
    border-color: #d8a657;
    background-color: #504945;
}

.campaign-btn:checked {
    border-color: #d8a657;
}

popover,
popover.background {
    background-color: #45403d;
    color: #d4be98;
    border: 1px solid #665c54;
    border-radius: 4px;
    padding: 4px;
}

popover list,
popover row {
    background-color: transparent;
    color: #d4be98;
}

popover row {
    border-radius: 3px;
    padding: 7px 10px;
}

popover row:hover {
    background-color: #504945;
}

popover row.selected-campaign {
    background-color: #3c3836;
}

popover row.selected-campaign .campaign-name {
    color: #d8a657;
}

.campaign-name {
    color: #d4be98;
    font-size: 12px;
}

.campaign-sub {
    color: #928374;
    font-size: 10px;
}

/* ── Buttons (base) ── */
button {
    background-image: none;
    background-color: #45403d;
    color: #d4be98;
    border: 1px solid #665c54;
    border-radius: 4px;
    padding: 7px 14px;
    transition: all 80ms;
    outline: none;
}

button:hover {
    background-color: #504945;
}

button:active {
    background-color: #3c3836;
}

button:disabled,
button:disabled label {
    background-image: none;
    background-color: #32302f;
    color: #504945;
    border-color: #3c3836;
}

/* ── Choose files button ── */
.choose-btn {
    color: #e78a4e;
    border-color: #665c54;
}

.choose-btn:hover {
    border-color: #e78a4e;
    background-color: #4a3e35;
}

/* ── Start button ── */
.start-btn {
    color: #a9b665;
    border-color: #a9b665;
    font-weight: bold;
    padding: 9px 20px;
    font-size: 13px;
}

.start-btn:hover {
    background-color: #a9b665;
    color: #1d2021;
}

.start-btn:disabled,
.start-btn:disabled label {
    background-color: #32302f;
    color: #3c3836;
    border-color: #3c3836;
}

/* ── Open folder button ── */
.open-btn {
    color: #7daea3;
    border-color: #7daea3;
    font-weight: bold;
    padding: 9px 20px;
    font-size: 13px;
}

.open-btn:hover {
    background-color: #7daea3;
    color: #1d2021;
}

.open-btn:disabled,
.open-btn:disabled label {
    background-color: #32302f;
    color: #3c3836;
    border-color: #3c3836;
}

/* ── Index to Memory button ── */
.memory-btn {
    color: #d3869b;
    border-color: #d3869b;
    font-weight: bold;
    padding: 9px 20px;
    font-size: 13px;
}

.memory-btn:hover {
    background-color: #d3869b;
    color: #1d2021;
}

.memory-btn:disabled,
.memory-btn:disabled label {
    background-color: #32302f;
    color: #3c3836;
    border-color: #3c3836;
}

/* ── Progress bar ── */
progressbar {
    border-radius: 4px;
    min-height: 18px;
}

progressbar trough {
    background-image: none;
    background-color: #3c3836;
    border: 1px solid #504945;
    border-radius: 4px;
    min-height: 18px;
}

progressbar progress {
    background-image: none;
    background-color: #a9b665;
    border-radius: 3px;
    min-height: 16px;
}

progressbar text {
    color: #1d2021;
    font-size: 10px;
    font-weight: bold;
}

/* ── Log / file list ── */
.log-view,
.log-view text {
    background-color: #1d2021;
    color: #928374;
    font-family: monospace;
    font-size: 11px;
}

.file-list,
.file-list text {
    background-color: #282828;
    color: #d4be98;
    font-size: 11px;
}

scrolledwindow {
    border: 1px solid #3c3836;
    border-radius: 4px;
}

separator {
    background-color: #3c3836;
    min-height: 1px;
    min-width: 1px;
}
"""


class MBTranscriberWindow(Gtk.Window):

    def __init__(self):
        super().__init__(title="Session Transcriber")
        self.set_default_size(620, 720)
        self.set_resizable(True)
        self.set_border_width(0)
        self.set_position(Gtk.WindowPosition.CENTER)

        self._selected_files = []
        self._trans_folder   = None
        self._session_num    = None
        # Maps filename -> resolved display label for file list
        self._file_labels    = {}
        # Campaign registry: every folder under 0.2_campaigns with a canon_aliases.json
        self._campaigns      = campaign_mod.list_campaigns()
        self._campaign       = self._campaigns[0][1] if self._campaigns else None

        self._apply_css()
        self._build_ui()
        self.show_all()
        self._open_btn.set_sensitive(False)
        self._start_btn.set_sensitive(False)
        self._memory_btn.set_sensitive(False)
        self._select_campaign(self._campaign.slug if self._campaign else None)
        self.present()

    # ── CSS ───────────────────────────────────────────────────────────────────

    def _apply_css(self):
        provider = Gtk.CssProvider()
        provider.load_from_data(CSS.encode("utf-8"))
        Gtk.StyleContext.add_provider_for_screen(
            Gdk.Screen.get_default(),
            provider,
            Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION,
        )

    # ── UI Construction ───────────────────────────────────────────────────────

    def _build_ui(self):
        root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=0)
        self.add(root)
        root.pack_start(self._make_header(),  False, False, 0)
        root.pack_start(Gtk.Separator(),      False, False, 0)
        root.pack_start(self._make_content(), True,  True,  0)
        root.pack_start(Gtk.Separator(),      False, False, 0)
        root.pack_start(self._make_footer(),  False, False, 0)

    def _make_header(self):
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
        box.set_margin_top(22)
        box.set_margin_bottom(18)
        box.set_margin_start(28)
        box.set_margin_end(28)

        title = Gtk.Label(label="SESSION TRANSCRIBER")
        title.get_style_context().add_class("title-label")
        title.set_halign(Gtk.Align.START)
        box.pack_start(title, False, False, 0)

        self._subtitle = Gtk.Label(label="session audio  ->  transcript pipeline")
        self._subtitle.get_style_context().add_class("subtitle-label")
        self._subtitle.set_halign(Gtk.Align.START)
        self._subtitle.set_ellipsize(Pango.EllipsizeMode.MIDDLE)
        self._subtitle.set_max_width_chars(76)
        box.pack_start(self._subtitle, False, False, 0)

        return box

    def _make_content(self):
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=22)
        box.set_margin_top(20)
        box.set_margin_bottom(20)
        box.set_margin_start(28)
        box.set_margin_end(28)
        box.pack_start(self._make_campaign_row(),     False, False, 0)
        box.pack_start(self._make_session_row(),      False, False, 0)
        box.pack_start(self._make_audio_section(),    False, False, 0)
        box.pack_start(self._make_progress_section(), False, False, 0)
        box.pack_start(self._make_log_section(),      True,  True,  0)
        return box

    def _make_campaign_row(self):
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)

        lbl = Gtk.Label(label="CAMPAIGN")
        lbl.get_style_context().add_class("section-label")
        lbl.set_halign(Gtk.Align.START)
        box.pack_start(lbl, False, False, 0)

        # MenuButton + Popover rather than ComboBoxText: see the CSS note above.
        self._campaign_btn = Gtk.MenuButton()
        self._campaign_btn.get_style_context().add_class("campaign-btn")

        btn_inner = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        self._campaign_btn_lbl = Gtk.Label(label="")
        self._campaign_btn_lbl.set_halign(Gtk.Align.START)
        self._campaign_btn_lbl.set_ellipsize(Pango.EllipsizeMode.MIDDLE)
        btn_inner.pack_start(self._campaign_btn_lbl, True, True, 0)
        btn_inner.pack_end(
            Gtk.Image.new_from_icon_name("pan-down-symbolic", Gtk.IconSize.BUTTON),
            False, False, 0)
        self._campaign_btn.add(btn_inner)

        self._campaign_popover = Gtk.Popover()
        self._campaign_popover.set_position(Gtk.PositionType.BOTTOM)

        listbox = Gtk.ListBox()
        listbox.set_selection_mode(Gtk.SelectionMode.NONE)
        self._campaign_rows = {}
        for slug, camp in self._campaigns:
            row = Gtk.ListBoxRow()
            row.campaign_slug = slug

            row_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2)
            name = Gtk.Label(label=camp.menu_label)
            name.get_style_context().add_class("campaign-name")
            name.set_halign(Gtk.Align.START)
            row_box.pack_start(name, False, False, 0)

            pcs = ", ".join(camp.pcs)
            sub = Gtk.Label(label=f"{slug}   ·   {pcs}" if pcs else slug)
            sub.get_style_context().add_class("campaign-sub")
            sub.set_halign(Gtk.Align.START)
            sub.set_ellipsize(Pango.EllipsizeMode.END)
            sub.set_max_width_chars(52)
            row_box.pack_start(sub, False, False, 0)

            row.add(row_box)
            listbox.add(row)
            self._campaign_rows[slug] = row

        listbox.connect("row-activated", self._on_campaign_row_activated)

        scroll = Gtk.ScrolledWindow()
        scroll.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
        scroll.set_max_content_height(280)
        scroll.set_propagate_natural_height(True)
        scroll.set_propagate_natural_width(True)
        scroll.add(listbox)

        self._campaign_popover.add(scroll)
        # The window's show_all() does not reach a MenuButton's popover, so the
        # popover's contents have to be shown explicitly or it pops up empty.
        scroll.show_all()
        self._campaign_btn.set_popover(self._campaign_popover)
        self._campaign_btn.set_sensitive(bool(self._campaigns))
        box.pack_start(self._campaign_btn, False, False, 0)

        self._campaign_hint = Gtk.Label(label="")
        self._campaign_hint.get_style_context().add_class("hint-label")
        self._campaign_hint.set_halign(Gtk.Align.START)
        self._campaign_hint.set_ellipsize(Pango.EllipsizeMode.MIDDLE)
        self._campaign_hint.set_max_width_chars(76)
        box.pack_start(self._campaign_hint, False, False, 0)

        # Shown only for a campaign with no review_only yet. Inline, not a modal:
        # it is a standing condition, not an event.
        self._review_banner = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=10)
        self._review_lbl = Gtk.Label(label="")
        self._review_lbl.get_style_context().add_class("warn-label")
        self._review_lbl.set_halign(Gtk.Align.START)
        self._review_lbl.set_line_wrap(True)
        self._review_lbl.set_max_width_chars(60)
        self._review_banner.pack_start(self._review_lbl, True, True, 0)

        self._review_btn = Gtk.Button(label="Seed review_only...")
        self._review_btn.get_style_context().add_class("choose-btn")
        self._review_btn.set_valign(Gtk.Align.CENTER)
        self._review_btn.connect("clicked", self._on_seed_review)
        self._review_banner.pack_end(self._review_btn, False, False, 0)

        box.pack_start(self._review_banner, False, False, 0)
        self._review_banner.set_no_show_all(True)
        self._review_banner.hide()

        if not self._campaigns:
            # setup_hint() distinguishes "vault set but empty" from "no vault
            # configured at all", and never raises, so a fresh install opens to
            # an instruction instead of a traceback.
            self._campaign_hint.set_text(campaign_mod.setup_hint())

        return box

    def _make_session_row(self):
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)

        lbl = Gtk.Label(label="SESSION NUMBER")
        lbl.get_style_context().add_class("section-label")
        lbl.set_halign(Gtk.Align.START)
        box.pack_start(lbl, False, False, 0)

        row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=12)

        self._session_entry = Gtk.Entry()
        self._session_entry.set_placeholder_text("e.g.  12")
        self._session_entry.set_width_chars(7)
        self._session_entry.set_max_length(4)
        self._session_entry.set_input_purpose(Gtk.InputPurpose.DIGITS)
        self._session_entry.connect("changed", self._on_session_changed)
        row.pack_start(self._session_entry, False, False, 0)

        self._session_preview = Gtk.Label(label="")
        self._session_preview.get_style_context().add_class("preview-label")
        self._session_preview.set_halign(Gtk.Align.START)
        row.pack_start(self._session_preview, False, False, 0)

        box.pack_start(row, False, False, 0)
        return box

    def _make_audio_section(self):
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)

        header_row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=0)
        lbl = Gtk.Label(label="AUDIO FILES")
        lbl.get_style_context().add_class("section-label")
        lbl.set_halign(Gtk.Align.START)
        header_row.pack_start(lbl, True, True, 0)

        self._file_count_lbl = Gtk.Label(label="No files selected")
        self._file_count_lbl.get_style_context().add_class("hint-label")
        header_row.pack_end(self._file_count_lbl, False, False, 0)
        box.pack_start(header_row, False, False, 0)

        scroll = Gtk.ScrolledWindow()
        scroll.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
        scroll.set_min_content_height(80)
        scroll.set_max_content_height(100)

        self._file_store = Gtk.ListStore(str)
        self._file_view  = Gtk.TreeView(model=self._file_store)
        self._file_view.get_style_context().add_class("file-list")
        self._file_view.set_headers_visible(False)
        self._file_view.set_can_focus(False)
        col = Gtk.TreeViewColumn("", Gtk.CellRendererText(), text=0)
        self._file_view.append_column(col)
        scroll.add(self._file_view)
        box.pack_start(scroll, False, False, 0)

        btn_row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=10)

        choose_btn = Gtk.Button(label="  Choose Audio Files...")
        choose_btn.get_style_context().add_class("choose-btn")
        choose_btn.connect("clicked", self._on_choose_files)
        btn_row.pack_start(choose_btn, False, False, 0)

        self._resolve_btn = Gtk.Button(label="  Resolve Speakers...")
        self._resolve_btn.get_style_context().add_class("choose-btn")
        self._resolve_btn.set_tooltip_text(
            "Name the players behind unrecognised Craig tracks.\n"
            "Saves them to the selected campaign's discord_names."
        )
        self._resolve_btn.connect("clicked", self._on_resolve_speakers)
        self._resolve_btn.set_sensitive(False)
        btn_row.pack_start(self._resolve_btn, False, False, 0)

        box.pack_start(btn_row, False, False, 0)

        return box

    def _make_progress_section(self):
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)

        lbl = Gtk.Label(label="PROGRESS")
        lbl.get_style_context().add_class("section-label")
        lbl.set_halign(Gtk.Align.START)
        box.pack_start(lbl, False, False, 0)

        self._progress = Gtk.ProgressBar()
        self._progress.set_fraction(0.0)
        self._progress.set_show_text(True)
        self._progress.set_text("Ready")
        box.pack_start(self._progress, False, False, 0)

        self._status_lbl = Gtk.Label(label="")
        self._status_lbl.get_style_context().add_class("status-label")
        self._status_lbl.set_halign(Gtk.Align.START)
        self._status_lbl.set_ellipsize(Pango.EllipsizeMode.MIDDLE)
        self._status_lbl.set_max_width_chars(72)
        box.pack_start(self._status_lbl, False, False, 0)

        return box

    def _make_log_section(self):
        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)

        lbl = Gtk.Label(label="OUTPUT")
        lbl.get_style_context().add_class("section-label")
        lbl.set_halign(Gtk.Align.START)
        box.pack_start(lbl, False, False, 0)

        scroll = Gtk.ScrolledWindow()
        scroll.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
        scroll.set_min_content_height(130)

        self._log_buf  = Gtk.TextBuffer()
        self._log_view = Gtk.TextView(buffer=self._log_buf)
        self._log_view.get_style_context().add_class("log-view")
        self._log_view.set_editable(False)
        self._log_view.set_cursor_visible(False)
        self._log_view.set_wrap_mode(Gtk.WrapMode.WORD_CHAR)
        self._log_view.set_left_margin(8)
        self._log_view.set_right_margin(8)
        self._log_view.set_top_margin(6)
        self._log_view.set_bottom_margin(6)
        scroll.add(self._log_view)

        box.pack_start(scroll, True, True, 0)
        return box

    def _make_footer(self):
        box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=12)
        box.set_margin_top(16)
        box.set_margin_bottom(18)
        box.set_margin_start(28)
        box.set_margin_end(28)

        self._start_btn = Gtk.Button(label="Start Transcription")
        self._start_btn.get_style_context().add_class("start-btn")
        self._start_btn.connect("clicked", self._on_start)
        box.pack_start(self._start_btn, True, True, 0)

        self._open_btn = Gtk.Button(label="Open Transcripts")
        self._open_btn.get_style_context().add_class("open-btn")
        self._open_btn.connect("clicked", self._on_open_folder)
        box.pack_start(self._open_btn, False, False, 0)

        self._memory_btn = Gtk.Button(label="Index to Memory")
        self._memory_btn.get_style_context().add_class("memory-btn")
        self._memory_btn.set_tooltip_text(
            "Run after the recap pass so alias corrections are in place.\n"
            "Chunks transcript.md and ingests it into ChromaDB."
        )
        self._memory_btn.connect("clicked", self._on_index_memory)
        box.pack_start(self._memory_btn, False, False, 0)

        return box

    # ── Campaign selection ────────────────────────────────────────────────────

    def _on_campaign_row_activated(self, listbox, row):
        # popdown() normally clears the button via the popover's "closed" signal;
        # setting it directly too means the button can't be left stuck looking
        # pressed if that signal is deferred.
        self._campaign_popover.popdown()
        self._campaign_btn.set_active(False)
        self._select_campaign(row.campaign_slug)

    def _select_campaign(self, slug):
        if slug:
            try:
                self._campaign = campaign_mod.load(slug)
            except campaign_mod.CampaignError as exc:
                self._append_log(f"ERROR: {exc}")
                return

        camp = self._campaign
        if camp is None:
            self._campaign_btn_lbl.set_text("No campaigns found")
            return

        self._campaign_btn_lbl.set_text(camp.menu_label)
        for row_slug, row in self._campaign_rows.items():
            ctx = row.get_style_context()
            if row_slug == camp.slug:
                ctx.add_class("selected-campaign")
            else:
                ctx.remove_class("selected-campaign")

        self._subtitle.set_text(
            f"{camp.label}"
            + (f"  ·  {camp.system}" if camp.system else "")
            + (f"  ·  {camp.world}" if camp.world else "")
        )
        self._campaign_hint.set_text(f"{camp.slug}   →   {camp.transcripts_dir}")
        self._campaign_btn.set_tooltip_text(
            f"slug:        {camp.slug}\n"
            f"aliases:     {camp.aliases_path}\n"
            f"audio:       {camp.audio_dir_base}\n"
            f"transcripts: {camp.transcripts_dir}\n"
            f"chunks:      {camp.chunks_dir}\n"
            f"chroma db:   {camp.db_path}\n"
            f"characters:  {', '.join(camp.pcs) or '(none)'}"
        )

        # Re-resolve any already-chosen files against the new campaign's roster.
        if self._selected_files:
            self._refresh_file_list()
        self._on_session_changed(self._session_entry)
        self._update_review_banner()

    @staticmethod
    def _discord_stem(fname):
        """Craig names tracks N-discordusername.ext; strip the track number."""
        base = os.path.splitext(fname)[0]
        return base.split("-", 1)[-1] if "-" in base else base

    # ── discord_names resolution ──────────────────────────────────────────────
    # Reads the SELECTED campaign's canon_aliases.json and maps raw Craig
    # filenames to speaker labels. Returns basename -> display string.
    # Unknown usernames get a warning marker and an offer to record them.

    def _resolve_discord_names(self, filenames):
        discord_names = self._campaign.discord_names if self._campaign else {}

        result = {}
        for fname in filenames:
            stem = self._discord_stem(fname)
            info = discord_names.get(stem)
            if isinstance(info, dict):
                role = info.get("role", "")
                char = info.get("character") or role
                player = info.get("player", "?")
                if role == "GM":
                    label = f"{fname}   →  GM ({player})"
                else:
                    label = f"{fname}   →  {char} ({player})"
            else:
                label = f"{fname}   →  ⚠ Unknown  (click Resolve Speakers)"

            result[fname] = label

        return result

    def _refresh_file_list(self):
        basenames = [os.path.basename(f) for f in self._selected_files]
        self._file_labels = self._resolve_discord_names(basenames)
        self._file_store.clear()
        for fname in basenames:
            self._file_store.append([self._file_labels.get(fname, fname)])

        n = len(self._selected_files)
        unknowns = sum(1 for v in self._file_labels.values() if "⚠" in v)
        count_text = f"{n} file{'s' if n != 1 else ''} selected"
        if unknowns:
            count_text += f"  —  {unknowns} unknown speaker(s)"
        else:
            count_text += "  —  all speakers resolved"
        self._file_count_lbl.set_text(count_text)
        self._resolve_btn.set_sensitive(bool(unknowns) and self._campaign is not None)

    # ── Event Handlers ────────────────────────────────────────────────────────

    def _on_session_changed(self, widget):
        text = widget.get_text().strip()
        clean = ''.join(c for c in text if c.isdigit())
        if clean != text:
            widget.set_text(clean)
            widget.set_position(-1)
            return
        if clean and int(clean) > 0 and self._campaign:
            n = int(clean)
            self._session_num   = clean
            self._trans_folder  = self._campaign.session_dir(n)
            self._session_preview.set_text(f"->  {campaign_mod.session_folder(n)}")
            # An already-transcribed session can be indexed without re-running
            # whisper, which is the normal flow: transcribe, recap pass, index.
            has_md = os.path.isfile(self._campaign.transcript_path(n))
            self._open_btn.set_sensitive(os.path.isdir(self._trans_folder))
            self._memory_btn.set_sensitive(has_md)
        else:
            self._session_num  = None
            self._trans_folder = None
            self._session_preview.set_text("")
            self._open_btn.set_sensitive(False)
            self._memory_btn.set_sensitive(False)
        self._refresh_start_btn()

    def _on_choose_files(self, widget):
        dialog = Gtk.FileChooserDialog(
            title="Select Craig Session Audio",
            parent=self,
            action=Gtk.FileChooserAction.OPEN,
        )
        dialog.add_button(Gtk.STOCK_CANCEL, Gtk.ResponseType.CANCEL)
        dialog.add_button(Gtk.STOCK_OPEN,   Gtk.ResponseType.OK)
        dialog.set_select_multiple(True)

        audio_filter = Gtk.FileFilter()
        audio_filter.set_name("Audio files (flac, mp3, wav, m4a, ogg, aac)")
        for pat in ("*.flac", "*.mp3", "*.wav", "*.m4a", "*.ogg", "*.aac", "*.wma"):
            audio_filter.add_pattern(pat)
        dialog.add_filter(audio_filter)

        all_filter = Gtk.FileFilter()
        all_filter.set_name("All files")
        all_filter.add_pattern("*")
        dialog.add_filter(all_filter)

        downloads = os.path.expanduser("~/Downloads")
        if os.path.isdir(downloads):
            dialog.set_current_folder(downloads)

        if dialog.run() == Gtk.ResponseType.OK:
            self._selected_files = sorted(dialog.get_filenames())
            self._refresh_file_list()

        dialog.destroy()
        self._refresh_start_btn()

    # ── Unknown speaker fallback ──────────────────────────────────────────────
    # Craig gives us raw Discord usernames. Anything not already in the selected
    # campaign's discord_names is flagged, and this dialog records it. It writes
    # to THAT campaign's canon_aliases.json, spliced in so the hand-maintained
    # key order, alignment and _comment fields survive untouched.

    def _on_resolve_speakers(self, widget):
        if not self._campaign:
            return
        unknown = [os.path.basename(f) for f in self._selected_files
                   if "⚠" in self._file_labels.get(os.path.basename(f), "")]
        if not unknown:
            return

        dialog = Gtk.Dialog(title="Resolve Unknown Speakers", parent=self, modal=True)
        dialog.add_button(Gtk.STOCK_CANCEL, Gtk.ResponseType.CANCEL)
        dialog.add_button("Save to campaign", Gtk.ResponseType.OK)
        dialog.set_default_size(560, -1)

        content = dialog.get_content_area()
        content.set_spacing(10)
        content.set_border_width(16)

        intro = Gtk.Label(
            label=f"These tracks are not in {self._campaign.slug}'s discord_names.\n"
                  f"Leave a row blank to skip it. Character blank means the GM.")
        intro.set_halign(Gtk.Align.START)
        intro.set_line_wrap(True)
        content.pack_start(intro, False, False, 0)

        grid = Gtk.Grid(column_spacing=10, row_spacing=8)
        for col, heading in enumerate(("track", "player", "character")):
            h = Gtk.Label(label=heading.upper())
            h.get_style_context().add_class("section-label")
            h.set_halign(Gtk.Align.START)
            grid.attach(h, col, 0, 1, 1)

        rows = []
        known_chars = self._campaign.pcs
        for i, fname in enumerate(unknown, start=1):
            stem = self._discord_stem(fname)
            name_lbl = Gtk.Label(label=stem)
            name_lbl.set_halign(Gtk.Align.START)
            name_lbl.set_ellipsize(Pango.EllipsizeMode.MIDDLE)
            name_lbl.set_max_width_chars(24)
            name_lbl.set_tooltip_text(fname)
            grid.attach(name_lbl, 0, i, 1, 1)

            player = Gtk.Entry()
            player.set_width_chars(12)
            player.set_placeholder_text("player name")
            grid.attach(player, 1, i, 1, 1)

            char = Gtk.ComboBoxText.new_with_entry()
            char.append_text("")            # blank == GM
            for pc in known_chars:
                char.append_text(pc)
            char.get_child().set_placeholder_text("blank = GM")
            char.get_child().set_width_chars(20)
            grid.attach(char, 2, i, 1, 1)

            rows.append((stem, player, char))

        content.pack_start(grid, False, False, 0)
        dialog.show_all()

        entries = {}
        if dialog.run() == Gtk.ResponseType.OK:
            for stem, player_entry, char_combo in rows:
                player = player_entry.get_text().strip()
                char = char_combo.get_child().get_text().strip()
                if not player and not char:
                    continue
                entries[stem] = {
                    "player": player or None,
                    "character": campaign_mod.normalize_character(char) if char else None,
                    "role": "Player" if char else "GM",
                }
        dialog.destroy()

        if not entries:
            return
        try:
            added = campaign_mod.append_object_entries(
                self._campaign.aliases_path, "discord_names", entries)
        except Exception as exc:
            self._append_log(f"ERROR: could not update {self._campaign.aliases_path}: {exc}")
            return

        self._append_log(f"Added to {self._campaign.slug} discord_names: {', '.join(added)}")
        self._campaign = campaign_mod.load(self._campaign.slug)
        self._refresh_file_list()
        self._refresh_start_btn()

    # ── review_only seeding offer ─────────────────────────────────────────────
    # review_only holds the manglings that collide with real English words, the
    # ones that must never be auto-replaced. It is the part of this system you
    # want in place BEFORE the first ingest, not after: Whisper will mangle names
    # like "Honky Boobs" and "Willem Dafoe" in ways worth catching up front.
    #
    # A campaign with none gets a standing banner offering a read-only report
    # pass over an existing transcript, and a confirmation before it indexes.

    def _update_review_banner(self):
        camp = self._campaign
        if camp is None or camp.has_review_only:
            self._review_banner.hide()
            return
        sessions = self._existing_sessions()
        if sessions:
            self._review_lbl.set_text(
                f"{camp.slug} has no review_only entries. "
                f"Run a report pass over S{sessions[-1]:02d} to find the manglings "
                f"that collide with real words, before the first ingest.")
            self._review_btn.set_sensitive(True)
        else:
            self._review_lbl.set_text(
                f"{camp.slug} has no review_only entries. Transcribe a session first, "
                f"then run the report pass before indexing.")
            self._review_btn.set_sensitive(False)
        self._review_banner.show()
        self._review_lbl.show()
        self._review_btn.show()

    def _on_seed_review(self, widget):
        sessions = self._existing_sessions()
        if not sessions:
            return
        target = int(self._session_num) if (
            self._session_num and int(self._session_num) in sessions) else sessions[-1]
        self._run_alias_report(target)

    def _confirm_index_without_review(self):
        """Called before ingesting a campaign that has no review_only yet."""
        camp = self._campaign
        if camp is None or camp.has_review_only:
            return True
        sessions = self._existing_sessions()

        dialog = Gtk.MessageDialog(
            transient_for=self, modal=True,
            message_type=Gtk.MessageType.WARNING,
            buttons=Gtk.ButtonsType.NONE,
            text=f"{camp.slug} has no review_only entries")
        dialog.format_secondary_text(
            "review_only is the list of manglings that collide with real English "
            "words, and it is worth having before the first ingest rather than "
            "after. A report pass is read-only and writes nothing to "
            "canon_aliases.json.")
        dialog.add_button("Cancel", Gtk.ResponseType.CANCEL)
        dialog.add_button("Index anyway", Gtk.ResponseType.NO)
        if sessions:
            dialog.add_button("Run report first", Gtk.ResponseType.YES)
            dialog.set_default_response(Gtk.ResponseType.YES)
        response = dialog.run()
        dialog.destroy()

        if response == Gtk.ResponseType.YES:
            self._run_alias_report(int(self._session_num) if self._session_num
                                   else sessions[-1])
            return False
        return response == Gtk.ResponseType.NO

    def _existing_sessions(self):
        """Session numbers that already have a transcript.md, ascending."""
        camp = self._campaign
        found = []
        if camp and os.path.isdir(camp.transcripts_dir):
            for entry in sorted(os.listdir(camp.transcripts_dir)):
                m = re.fullmatch(r"S(\d+)", entry)
                if m and os.path.isfile(camp.transcript_path(int(m.group(1)))):
                    found.append(int(m.group(1)))
        return sorted(found)

    def _run_alias_report(self, session):
        camp = self._campaign
        out = os.path.join(camp.session_dir(session), "alias_candidates.md")
        self._append_log(f"Running alias-candidate report on {camp.slug} S{session:02d}...")
        r = subprocess.run(
            [sys.executable, CHUNKS_SCRIPT, "--campaign", camp.slug,
             "--session", str(session), "--report", "-o", out],
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        for line in r.stdout.splitlines():
            self._append_log(line)
        if r.returncode == 0 and os.path.isfile(out):
            self._append_log(f"Report: {out}")
            self._set_status("Alias report ready. Nothing was written to canon_aliases.json.")
            subprocess.Popen(["xdg-open", out])
        else:
            self._set_status("Alias report failed, see output above.")

    def _on_start(self, widget):
        session_num = self._session_entry.get_text().strip()
        if not session_num or not session_num.isdigit() or not self._campaign:
            return

        self._session_num  = session_num
        audio_dir = self._campaign.audio_dir(int(session_num))
        trans_dir = self._campaign.session_dir(int(session_num))
        self._trans_folder = trans_dir

        self._start_btn.set_sensitive(False)
        self._session_entry.set_sensitive(False)
        self._open_btn.set_sensitive(False)
        self._memory_btn.set_sensitive(False)
        self._log_buf.set_text("")
        self._set_progress(0.0, "Starting...")

        threading.Thread(
            target=self._run_pipeline,
            args=(session_num, audio_dir, trans_dir),
            daemon=True,
        ).start()

    def _on_open_folder(self, widget):
        if self._trans_folder and os.path.isdir(self._trans_folder):
            subprocess.Popen(["xdg-open", self._trans_folder])

    def _on_index_memory(self, widget):
        if not self._trans_folder or not self._session_num or not self._campaign:
            return
        # Re-read canon_aliases.json so hand edits made since the campaign was
        # selected (new aliases, a freshly seeded review_only) take effect here
        # without restarting the GUI.
        try:
            self._campaign = campaign_mod.load(self._campaign.slug)
            self._update_review_banner()
        except campaign_mod.CampaignError as exc:
            self._append_log(f"ERROR: {exc}")
            return
        if not self._confirm_index_without_review():
            return

        self._memory_btn.set_sensitive(False)
        self._start_btn.set_sensitive(False)
        self._log_buf.set_text("")
        self._set_progress(0.0, "Starting memory indexing...")
        self._set_status("Chunking transcript and ingesting to ChromaDB...")

        threading.Thread(
            target=self._run_index_pipeline,
            args=(self._session_num, self._trans_folder),
            daemon=True,
        ).start()

    def _refresh_start_btn(self):
        num   = self._session_entry.get_text().strip()
        ready = (self._campaign is not None
                 and num.isdigit() and int(num) > 0
                 and len(self._selected_files) > 0)
        self._start_btn.set_sensitive(ready)

    # ── Pipeline Thread — Transcription ───────────────────────────────────────

    def _run_pipeline(self, session_num, audio_dir, trans_dir):
        try:
            if not os.path.isfile(SCRIPT_PATH):
                self._append_log(f"ERROR: Script not found: {SCRIPT_PATH}")
                self._set_status(f"Missing: {SCRIPT_PATH}")
                return

            self._set_status(f"Creating Session {session_num} folders...")
            os.makedirs(audio_dir, exist_ok=True)
            os.makedirs(trans_dir, exist_ok=True)
            self._append_log(f"Created: {audio_dir}")
            self._append_log(f"Created: {trans_dir}")

            self._set_status("Moving audio files to session folder...")
            self._set_progress(0.04, "Moving files...")
            for src in self._selected_files:
                dst = os.path.join(audio_dir, os.path.basename(src))
                shutil.move(src, dst)
                self._append_log(f"Moved:  {os.path.basename(src)}")
            self._append_log("")

            total_files = len(self._selected_files)

            self._set_progress(0.08, f"Transcribing 0 / {total_files}...")
            self._set_status("Starting whisper transcription...")

            # --campaign lets transcribe.sh resolve the alias file and keep the
            # whisper working files out of the vault; -i/-o are passed explicitly
            # because the GUI has already moved the audio into place.
            cmd  = ["bash", SCRIPT_PATH,
                    "--campaign", self._campaign.slug,
                    "--session", str(int(session_num)),
                    "-i", audio_dir, "-o", trans_dir]
            proc = subprocess.Popen(
                cmd,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                bufsize=1,
            )

            completed = 0

            for raw_line in proc.stdout:
                line = raw_line.rstrip()
                if not line:
                    continue

                if line.startswith("PROGRESS:"):
                    parts = line.split(":", 3)
                    tag   = parts[1] if len(parts) > 1 else ""

                    if tag == "start":
                        n = int(parts[2]) if len(parts) > 2 and parts[2].isdigit() else total_files
                        total_files = n
                        self._set_progress(0.08, f"Transcribing 0 / {n}...")

                    elif tag == "file":
                        idx_str, total_str = (parts[2].split("/") + ["1"])[:2]
                        idx   = int(idx_str)
                        total = int(total_str)
                        fname = parts[3] if len(parts) > 3 else ""
                        frac  = 0.08 + (idx - 1) / total * 0.82
                        self._set_progress(frac, f"File {idx} / {total}:  {fname}")
                        self._set_status(f"Transcribing:  {fname}")

                    elif tag == "done":
                        idx_str, total_str = (parts[2].split("/") + ["1"])[:2]
                        idx   = int(idx_str)
                        total = int(total_str)
                        completed = idx
                        frac  = 0.08 + idx / total * 0.82
                        self._set_progress(frac, f"Done {idx} / {total} files")

                    elif tag == "complete":
                        self._set_progress(0.92, "Post-processing...")
                        self._set_status("Merging and formatting transcripts...")

                    elif tag == "postdone":
                        self._set_progress(1.0, "Complete!")
                        self._set_status("")

                else:
                    self._append_log(line)

            proc.wait()

            self._set_progress(1.0, "Complete!")
            self._set_status(f"Session {session_num} done  --  {completed} file(s) transcribed.")
            self._append_log("")
            self._append_log(f"Transcripts saved to:  {trans_dir}")
            GLib.idle_add(self._open_btn.set_sensitive, True)

            # Enable Index to Memory if transcript.md was produced
            md_path = os.path.join(trans_dir, "transcript.md")
            if os.path.isfile(md_path):
                self._append_log("")
                self._append_log("transcript.md found. Run 'Index to Memory' after your recap pass.")
                GLib.idle_add(self._memory_btn.set_sensitive, True)

        except Exception as exc:
            err = traceback.format_exc()
            self._append_log(f"ERROR: {exc}")
            self._set_status(f"Error: {exc}")
            self._set_progress(0.0, "Error")
            _write_startup_log(f"Pipeline error:\n{err}")

        finally:
            GLib.idle_add(self._start_btn.set_sensitive, True)
            GLib.idle_add(self._session_entry.set_sensitive, True)

    # ── Pipeline Thread — Memory Indexing ─────────────────────────────────────
    # Runs AFTER the recap pass (manual trigger). Two steps:
    #   1. transcript_to_chunks.py  — parses transcript.md into a JSONL chunk file
    #   2. chroma_memory.py ingest  — upserts the chunks into the ChromaDB collection
    #
    # The session number is passed to the chunker so it can embed session metadata
    # in each chunk (used by chroma_memory.py query filters --from / --char).

    def _run_index_pipeline(self, session_num, trans_dir):
        try:
            camp       = self._campaign
            session    = int(session_num)
            md_path    = camp.transcript_path(session)
            jsonl_path = camp.chunks_path(session)

            if not os.path.isfile(md_path):
                self._append_log(f"ERROR: transcript.md not found in {trans_dir}")
                self._set_status("Index failed: transcript.md missing")
                self._set_progress(0.0, "Error")
                GLib.idle_add(self._memory_btn.set_sensitive, True)
                return

            if not os.path.isfile(CHUNKS_SCRIPT):
                self._append_log(f"ERROR: chunker not found: {CHUNKS_SCRIPT}")
                self._set_status("Index failed: transcript_to_chunks.py missing")
                self._set_progress(0.0, "Error")
                GLib.idle_add(self._memory_btn.set_sensitive, True)
                return

            if not os.path.isfile(CHROMA_SCRIPT):
                self._append_log(f"ERROR: chroma_memory.py not found: {CHROMA_SCRIPT}")
                self._set_status("Index failed: chroma_memory.py missing")
                self._set_progress(0.0, "Error")
                GLib.idle_add(self._memory_btn.set_sensitive, True)
                return

            # Step 1: chunk
            # The chunker writes the JSONL into the campaign's chunks folder itself
            # and reports progress on stderr.
            self._append_log(f"Chunking {camp.slug} S{session:02d}...")
            self._set_progress(0.1, "Chunking...")

            today = datetime.date.today().isoformat()
            chunk_cmd = [
                sys.executable, CHUNKS_SCRIPT,
                "--campaign", camp.slug,
                "--session", str(session),
                "--date", today,
                "-o", jsonl_path,
            ]
            r1 = subprocess.run(
                chunk_cmd,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
            # stderr has the human-readable progress line ("57 chunks from 1381 utterances...")
            for line in r1.stderr.splitlines():
                self._append_log(line)

            if r1.returncode != 0:
                self._append_log(f"ERROR: chunker exited with code {r1.returncode}")
                self._set_status("Chunking failed — see output above")
                self._set_progress(0.0, "Error")
                GLib.idle_add(self._memory_btn.set_sensitive, True)
                return

            if not os.path.isfile(jsonl_path) or os.path.getsize(jsonl_path) == 0:
                self._append_log(f"ERROR: JSONL empty or not produced: {jsonl_path}")
                self._set_status("Chunking failed — JSONL not produced")
                self._set_progress(0.0, "Error")
                GLib.idle_add(self._memory_btn.set_sensitive, True)
                return

            self._set_progress(0.5, "Chunking complete. Ingesting to ChromaDB...")
            self._append_log("")

            # Step 2: ingest into THIS campaign's database. One store per campaign,
            # so one campaign's query can never surface another's content.
            self._append_log(f"Ingesting {os.path.basename(jsonl_path)} "
                             f"into {camp.db_path}...")

            ingest_cmd = [
                sys.executable, CHROMA_SCRIPT,
                "ingest", "--campaign", camp.slug, jsonl_path,
            ]
            r2 = subprocess.run(
                ingest_cmd,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
            )
            for line in r2.stdout.splitlines():
                self._append_log(line)

            if r2.returncode != 0:
                self._append_log(f"ERROR: chroma_memory.py exited with code {r2.returncode}")
                self._set_status("Ingest failed — see output above")
                self._set_progress(0.0, "Error")
                GLib.idle_add(self._memory_btn.set_sensitive, True)
                return

            self._set_progress(1.0, "Indexed!")
            self._set_status(f"{camp.slug} S{session:02d} indexed to ChromaDB.")
            self._append_log("")
            self._append_log(f"Done. {jsonl_path}")

        except Exception as exc:
            err = traceback.format_exc()
            self._append_log(f"ERROR: {exc}")
            self._set_status(f"Index error: {exc}")
            self._set_progress(0.0, "Error")
            _write_startup_log(f"Index pipeline error:\n{err}")
            GLib.idle_add(self._memory_btn.set_sensitive, True)

        finally:
            GLib.idle_add(self._start_btn.set_sensitive, True)

    # ── Thread-safe UI helpers ─────────────────────────────────────────────────

    def _set_status(self, text: str):
        GLib.idle_add(self._status_lbl.set_text, text)

    def _set_progress(self, fraction: float, label: str = ""):
        def _do():
            self._progress.set_fraction(max(0.0, min(1.0, fraction)))
            if label:
                self._progress.set_text(label)
        GLib.idle_add(_do)

    def _append_log(self, text: str):
        def _do():
            end  = self._log_buf.get_end_iter()
            self._log_buf.insert(end, text + "\n")
            mark = self._log_buf.get_insert()
            self._log_view.scroll_mark_onscreen(mark)
        GLib.idle_add(_do)


# ── Entry point ───────────────────────────────────────────────────────────────

def main():
    _write_startup_log("main() called")
    try:
        _write_startup_log("Creating window...")
        win = MBTranscriberWindow()
        _write_startup_log("Window created, entering Gtk.main()")
        win.connect("destroy", Gtk.main_quit)
        Gtk.main()
        _write_startup_log("Gtk.main() returned (window closed)")
    except Exception as exc:
        _write_startup_log(f"FATAL: {exc}\n{traceback.format_exc()}")
        sys.exit(1)


if __name__ == "__main__":
    main()
