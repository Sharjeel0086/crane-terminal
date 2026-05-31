//! Find in Files — ⌘⇧F. Project-wide text search using the pure-Rust
//! `ignore` + `grep-searcher` stack (the same engine ripgrep itself
//! uses, statically linked — no external binary). Streams results
//! into the modal as they're found and renders an inline preview of
//! the focused match.
//!
//! Lifecycle:
//!   - `App::find_in_files` holds the modal state when open; `None`
//!     when closed.
//!   - Typing in the query box bumps `last_query_at`; once 150 ms
//!     idle passes, the next render kicks a fresh worker thread and
//!     bumps `search_token`. Older workers compare their token before
//!     pushing into `results` and bail silently on mismatch.

use crate::state::App;
use crate::theme;
use egui::{Color32, Key, Modifiers, RichText};
use grep_matcher::Matcher;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{Searcher, Sink, SinkMatch};
use ignore::WalkBuilder;
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const DEBOUNCE: Duration = Duration::from_millis(150);
const MAX_RESULTS: usize = 1000;
const PREVIEW_CONTEXT: usize = 10;
const MODAL_W: f32 = 880.0;
const MODAL_H: f32 = 620.0;
const QUERY_RIGHT_PAD: f32 = 230.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SearchScope {
    AllProjects,
    ActiveProject,
    ActiveWorkspace,
}

impl SearchScope {
    fn label(self) -> &'static str {
        match self {
            SearchScope::AllProjects => "All Projects",
            SearchScope::ActiveProject => "Project",
            SearchScope::ActiveWorkspace => "Workspace",
        }
    }
    fn tooltip(self) -> &'static str {
        match self {
            SearchScope::AllProjects => "Search every open project",
            SearchScope::ActiveProject => "Search the active project (all its workspaces)",
            SearchScope::ActiveWorkspace => "Search only the active workspace",
        }
    }
}

pub struct SearchMatch {
    pub path: PathBuf,
    pub display_path: String,
    pub line: u32,
    pub byte_start: usize,
    pub byte_end: usize,
    pub line_text: String,
}

pub struct SearchResults {
    pub matches: Vec<SearchMatch>,
    pub files_seen: std::collections::HashSet<PathBuf>,
    pub truncated: bool,
    pub running: bool,
    pub error: Option<String>,
    pub token: u64,
}

impl SearchResults {
    fn new(token: u64) -> Self {
        Self {
            matches: Vec::new(),
            files_seen: std::collections::HashSet::new(),
            truncated: false,
            running: false,
            error: None,
            token,
        }
    }
}

pub struct FindInFilesState {
    pub query: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
    pub file_mask: String,
    pub scope: SearchScope,
    pub results: Arc<Mutex<SearchResults>>,
    pub selected: usize,
    pub last_query_at: Option<Instant>,
    pub pending_kick: bool,
    pub search_token: u64,
    pub focus_input: bool,
    pub cancel_flag: Option<Arc<AtomicBool>>,
    pub preview_cache: Option<(PathBuf, Vec<String>)>,
}

impl Default for FindInFilesState {
    fn default() -> Self {
        Self {
            query: String::new(),
            case_sensitive: false,
            whole_word: false,
            regex: false,
            file_mask: String::new(),
            scope: SearchScope::AllProjects,
            results: Arc::new(Mutex::new(SearchResults::new(0))),
            selected: 0,
            last_query_at: None,
            pending_kick: false,
            search_token: 0,
            focus_input: true,
            cancel_flag: None,
            preview_cache: None,
        }
    }
}

pub fn open(app: &mut App) {
    if app.find_in_files.is_some() {
        if let Some(s) = app.find_in_files.as_mut() {
            s.focus_input = true;
        }
        return;
    }
    app.find_in_files = Some(FindInFilesState::default());
}

pub fn close(app: &mut App) {
    if let Some(s) = app.find_in_files.take()
        && let Some(f) = s.cancel_flag
    {
        f.store(true, Ordering::Relaxed);
    }
}

pub fn render(ctx: &egui::Context, app: &mut App) {
    if app.find_in_files.is_none() {
        return;
    }
    let mut close_requested = false;
    let mut open_request: Option<(PathBuf, u32, usize)> = None;
    let roots = collect_roots(app);

    let mut clicked_x = false;
    egui::Window::new("Find in Files")
        .id(egui::Id::new("crane_find_in_files"))
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, -40.0])
        .fixed_size(egui::vec2(MODAL_W, MODAL_H))
        .frame(
            egui::Frame::popup(&ctx.global_style())
                .inner_margin(egui::Margin::same(14)),
        )
        .show(ctx, |ui| {
            let Some(state) = app.find_in_files.as_mut() else {
                return;
            };
            ui.set_min_width(MODAL_W - 28.0);
            ui.set_max_width(MODAL_W - 28.0);
            let th = theme::current();

            render_header(ui, state, &mut clicked_x);
            ui.add_space(8.0);
            render_query_row(ui, state, &th);
            ui.add_space(4.0);
            render_mask_row(ui, state, &th);
            ui.add_space(6.0);
            render_scope_row(ui, state, &th);
            ui.add_space(6.0);
            ui.separator();

            let should_kick = state
                .last_query_at
                .map(|t| t.elapsed() >= DEBOUNCE)
                .unwrap_or(false)
                && state.pending_kick;
            if should_kick {
                state.pending_kick = false;
                spawn_search(state, &roots, ctx.clone());
            }
            if state.pending_kick {
                ctx.request_repaint_after(DEBOUNCE);
            }

            let (total, truncated, running, error) = {
                let g = state.results.lock();
                (g.matches.len(), g.truncated, g.running, g.error.clone())
            };

            let remaining = ui.available_height();
            let list_h = (remaining * 0.42).max(140.0);
            let preview_h = (remaining - list_h - 28.0).max(120.0);

            ui.allocate_ui(egui::vec2(MODAL_W - 28.0, list_h), |ui| {
                render_results_list(ui, state, &th, &mut open_request);
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(status_line(total, truncated, running, error.as_deref(),
                        state.query.is_empty()))
                        .size(11.0)
                        .color(th.text_muted.to_color32()),
                );
            });
            ui.add_space(2.0);
            ui.allocate_ui(egui::vec2(MODAL_W - 28.0, preview_h), |ui| {
                render_preview(ui, state, &th);
            });
        });

    if clicked_x {
        close_requested = true;
    }

    // Keyboard nav. The shortcuts.rs global handler consumes Esc/⌘W to
    // close us; here we only handle navigation keys.
    if let Some(state) = app.find_in_files.as_mut() {
        let (up, down, enter, page_up, page_down) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::PageUp),
                i.consume_key(Modifiers::NONE, Key::PageDown),
            )
        });
        let total = state.results.lock().matches.len();
        if total > 0 {
            if up && state.selected > 0 {
                state.selected -= 1;
                state.preview_cache = None;
            }
            if down && state.selected + 1 < total {
                state.selected += 1;
                state.preview_cache = None;
            }
            if page_up {
                state.selected = state.selected.saturating_sub(10);
                state.preview_cache = None;
            }
            if page_down {
                state.selected = (state.selected + 10).min(total - 1);
                state.preview_cache = None;
            }
            if enter {
                let guard = state.results.lock();
                if let Some(m) = guard.matches.get(state.selected) {
                    open_request = Some((m.path.clone(), m.line.saturating_sub(1), m.byte_start));
                }
            }
        }
    }

    if close_requested {
        close(app);
        return;
    }

    if let Some((path, line, _col)) = open_request {
        let path_str = path.to_string_lossy().to_string();
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&path_str)
            .to_string();
        let inside_workspace = app
            .active_workspace_path()
            .is_some_and(|ws| path.starts_with(ws));
        app.open_file_into_active_layout(
            ctx,
            path_str.clone(),
            name,
            content,
            false,
            !inside_workspace,
        );
        if let Some(layout) = app.active_layout() {
            for (_, pane) in layout.panes.iter_mut() {
                if let crate::state::layout::PaneContent::Files(files) = &mut pane.content
                    && let Some(idx) = files.tabs.iter().position(|t| {
                        matches!(t, crate::state::layout::TabKind::File(ft) if ft.path == path_str)
                    })
                {
                    files.active = idx;
                    if let Some(ft) = files.tabs[idx].as_file_mut() {
                        ft.pending_cursor = Some((line, 0));
                    }
                    break;
                }
            }
        }
        close(app);
    }
}

fn render_header(ui: &mut egui::Ui, state: &FindInFilesState, clicked_x: &mut bool) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Find in Files").size(14.0).strong());
        let (total, files, truncated) = {
            let g = state.results.lock();
            (g.matches.len(), g.files_seen.len(), g.truncated)
        };
        if !state.query.is_empty() {
            let s = if truncated {
                format!("{}+ matches in {}+ files", total, files)
            } else {
                format!("{} matches in {} files", total, files)
            };
            ui.label(
                RichText::new(s)
                    .size(11.0)
                    .color(theme::current().text_muted.to_color32()),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(
                    egui::Button::new(RichText::new(egui_phosphor::regular::X).size(13.0))
                        .frame(false)
                        .min_size(egui::vec2(22.0, 22.0)),
                )
                .on_hover_text("Close (Esc)")
                .clicked()
            {
                *clicked_x = true;
            }
        });
    });
}

fn render_query_row(ui: &mut egui::Ui, state: &mut FindInFilesState, th: &theme::Theme) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(egui_phosphor::regular::MAGNIFYING_GLASS)
                .color(th.text_muted.to_color32()),
        );
        let id = egui::Id::new("find_in_files_query");
        let input_w = (MODAL_W - 28.0 - QUERY_RIGHT_PAD).max(200.0);
        let resp = ui.add_sized(
            egui::vec2(input_w, 26.0),
            egui::TextEdit::singleline(&mut state.query)
                .id(id)
                .hint_text("Find…")
                .desired_width(input_w),
        );
        if state.focus_input {
            resp.request_focus();
            state.focus_input = false;
        }
        if resp.changed() {
            state.last_query_at = Some(Instant::now());
            state.pending_kick = true;
            state.selected = 0;
        }
        let mut kick = false;
        if toggle_pill(ui, "Aa", &mut state.case_sensitive, "Case sensitive") {
            kick = true;
        }
        if toggle_pill(ui, "W", &mut state.whole_word, "Whole word") {
            kick = true;
        }
        if toggle_pill(ui, ".*", &mut state.regex, "Regex") {
            kick = true;
        }
        if kick {
            state.last_query_at = Some(Instant::now());
            state.pending_kick = true;
            state.selected = 0;
        }
    });
}

fn render_mask_row(ui: &mut egui::Ui, state: &mut FindInFilesState, th: &theme::Theme) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new("File mask")
                .size(11.0)
                .color(th.text_muted.to_color32()),
        );
        let resp = ui.add_sized(
            egui::vec2(220.0, 22.0),
            egui::TextEdit::singleline(&mut state.file_mask)
                .id(egui::Id::new("find_in_files_mask"))
                .hint_text("*.rs, *.toml")
                .desired_width(220.0),
        );
        if resp.changed() {
            state.last_query_at = Some(Instant::now());
            state.pending_kick = true;
        }
        ui.label(
            RichText::new("(comma-separated globs)")
                .size(10.0)
                .color(th.text_muted.to_color32()),
        );
    });
}

fn toggle_pill(ui: &mut egui::Ui, label: &str, value: &mut bool, tip: &str) -> bool {
    let th = theme::current();
    let bg = if *value {
        th.accent.to_color32()
    } else {
        th.surface.to_color32()
    };
    let fg = if *value {
        Color32::WHITE
    } else {
        th.text_muted.to_color32()
    };
    let resp = ui.add(
        egui::Button::new(RichText::new(label).size(11.0).color(fg))
            .fill(bg)
            .min_size(egui::vec2(30.0, 24.0))
            .corner_radius(4.0)
            .stroke(egui::Stroke::new(1.0, th.border.to_color32())),
    );
    let clicked = resp.on_hover_text(tip).clicked();
    if clicked {
        *value = !*value;
    }
    clicked
}

fn render_scope_row(ui: &mut egui::Ui, state: &mut FindInFilesState, th: &theme::Theme) {
    ui.horizontal(|ui| {
        for scope in [
            SearchScope::AllProjects,
            SearchScope::ActiveProject,
            SearchScope::ActiveWorkspace,
        ] {
            let active = state.scope == scope;
            let bg = if active {
                th.accent.to_color32().linear_multiply(0.25)
            } else {
                Color32::TRANSPARENT
            };
            let fg = if active {
                th.text_hover.to_color32()
            } else {
                th.text_muted.to_color32()
            };
            let resp = ui.add(
                egui::Button::new(RichText::new(scope.label()).size(12.0).color(fg))
                    .fill(bg)
                    .corner_radius(4.0)
                    .stroke(egui::Stroke::new(
                        1.0,
                        if active {
                            th.accent.to_color32()
                        } else {
                            th.border.to_color32()
                        },
                    ))
                    .min_size(egui::vec2(0.0, 24.0)),
            );
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let resp = resp.on_hover_text(scope.tooltip());
            if resp.clicked() && !active {
                state.scope = scope;
                state.last_query_at = Some(Instant::now());
                state.pending_kick = true;
                state.selected = 0;
            }
        }
    });
}

fn render_results_list(
    ui: &mut egui::Ui,
    state: &mut FindInFilesState,
    th: &theme::Theme,
    open_request: &mut Option<(PathBuf, u32, usize)>,
) {
    let snapshot = state.results.lock();
    let len = snapshot.matches.len();
    if len == 0 {
        drop(snapshot);
        let msg = if state.query.is_empty() {
            "Type to search across files"
        } else {
            let g = state.results.lock();
            if g.running {
                "Searching…"
            } else if g.error.is_some() {
                "Search failed"
            } else {
                "No matches"
            }
        };
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new(msg)
                    .size(12.0)
                    .color(th.text_muted.to_color32()),
            );
        });
        return;
    }
    let row_h = 22.0;
    let selected = state.selected.min(len - 1);
    let prev_selected = ui.ctx().memory(|m| {
        m.data
            .get_temp::<usize>(egui::Id::new("find_in_files_prev_sel"))
            .unwrap_or(usize::MAX)
    });
    let scroll_to = if prev_selected != selected {
        ui.ctx().memory_mut(|m| {
            m.data
                .insert_temp(egui::Id::new("find_in_files_prev_sel"), selected);
        });
        Some(selected as f32 * row_h)
    } else {
        None
    };
    let mut clicked_idx: Option<usize> = None;
    let mut double_clicked: Option<usize> = None;
    let mut area = egui::ScrollArea::vertical()
        .id_salt("find_in_files_results")
        .auto_shrink([false; 2]);
    if let Some(y) = scroll_to {
        area = area.vertical_scroll_offset((y - 80.0).max(0.0));
    }
    area.show_rows(ui, row_h, len, |ui, range| {
        for i in range {
            let m = &snapshot.matches[i];
            let row_w = ui.available_width();
            let (rect, resp) = ui.allocate_exact_size(
                egui::vec2(row_w, row_h),
                egui::Sense::click(),
            );
            let is_sel = i == selected;
            if is_sel {
                ui.painter().rect_filled(rect, 3.0, th.row_active.to_color32());
            } else if resp.hovered() {
                ui.painter().rect_filled(rect, 3.0, th.row_hover.to_color32());
            }
            let line_color = if is_sel {
                th.text_hover.to_color32()
            } else {
                th.text.to_color32()
            };
            let trimmed = m.line_text.trim_end_matches(['\n', '\r']);
            let safe_start = byte_floor(trimmed, m.byte_start);
            let safe_end = byte_floor(trimmed, m.byte_end).max(safe_start);
            let mut job = egui::text::LayoutJob::default();
            let fmt = egui::text::TextFormat {
                color: line_color,
                font_id: egui::FontId::monospace(12.0),
                ..Default::default()
            };
            let mut hit_fmt = fmt.clone();
            hit_fmt.background = th.accent.to_color32().linear_multiply(0.5);
            hit_fmt.color = Color32::WHITE;
            job.append(&trimmed[..safe_start], 0.0, fmt.clone());
            job.append(&trimmed[safe_start..safe_end], 0.0, hit_fmt);
            job.append(&trimmed[safe_end..], 0.0, fmt);

            // Right column: path + line number
            let path_label = format!("{}  :{}", m.display_path, m.line);
            let path_galley = ui.painter().layout_no_wrap(
                path_label.clone(),
                egui::FontId::proportional(11.0),
                th.text_muted.to_color32(),
            );
            let path_w = path_galley.size().x.min(row_w * 0.5);
            let text_w = (row_w - path_w - 24.0).max(80.0);

            let text_galley = {
                let mut j = job.clone();
                j.wrap.max_width = text_w;
                j.wrap.max_rows = 1;
                j.wrap.break_anywhere = false;
                ui.fonts_mut(|f| f.layout_job(j))
            };
            let text_pos = egui::pos2(rect.left() + 8.0, rect.center().y - text_galley.size().y * 0.5);
            ui.painter().galley(text_pos, text_galley, line_color);
            let path_pos = egui::pos2(
                rect.right() - path_w - 8.0,
                rect.center().y - path_galley.size().y * 0.5,
            );
            ui.painter().galley(path_pos, path_galley, th.text_muted.to_color32());

            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if resp.clicked() {
                clicked_idx = Some(i);
            }
            if resp.double_clicked() {
                double_clicked = Some(i);
            }
        }
    });
    drop(snapshot);
    if let Some(i) = clicked_idx {
        state.selected = i;
        state.preview_cache = None;
    }
    if let Some(i) = double_clicked {
        let guard = state.results.lock();
        if let Some(m) = guard.matches.get(i) {
            *open_request = Some((m.path.clone(), m.line.saturating_sub(1), m.byte_start));
        }
    }
}

fn render_preview(ui: &mut egui::Ui, state: &mut FindInFilesState, th: &theme::Theme) {
    let (path, line) = {
        let guard = state.results.lock();
        match guard.matches.get(state.selected) {
            Some(m) => (m.path.clone(), m.line),
            None => return,
        }
    };
    let lines = match &state.preview_cache {
        Some((cached, lines)) if cached == &path => lines.clone(),
        _ => {
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            let v: Vec<String> = content.lines().map(|s| s.to_string()).collect();
            state.preview_cache = Some((path.clone(), v.clone()));
            v
        }
    };
    if lines.is_empty() {
        return;
    }
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(path.to_string_lossy())
                .size(11.0)
                .color(th.text_hover.to_color32()),
        );
    });
    ui.add_space(2.0);
    let target = line.saturating_sub(1) as usize;
    let start = target.saturating_sub(PREVIEW_CONTEXT);
    let end = (target + PREVIEW_CONTEXT + 1).min(lines.len());
    let hit_span = {
        let guard = state.results.lock();
        guard
            .matches
            .get(state.selected)
            .map(|m| (m.byte_start, m.byte_end))
    };
    egui::ScrollArea::vertical()
        .id_salt("find_in_files_preview")
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            for (idx, l) in lines[start..end].iter().enumerate() {
                let lineno = start + idx + 1;
                let is_hit_line = (lineno as u32) == line;
                let row_w = ui.available_width();
                let (rect, _resp) = ui.allocate_exact_size(
                    egui::vec2(row_w, 18.0),
                    egui::Sense::hover(),
                );
                if is_hit_line {
                    ui.painter()
                        .rect_filled(rect, 0.0, th.row_active.to_color32());
                }
                let num_galley = ui.painter().layout_no_wrap(
                    format!("{:>5}", lineno),
                    egui::FontId::monospace(11.0),
                    th.text_muted.to_color32(),
                );
                ui.painter().galley(
                    egui::pos2(rect.left() + 4.0, rect.center().y - num_galley.size().y * 0.5),
                    num_galley,
                    th.text_muted.to_color32(),
                );

                let mut job = egui::text::LayoutJob::default();
                let fmt = egui::text::TextFormat {
                    color: th.text.to_color32(),
                    font_id: egui::FontId::monospace(12.0),
                    ..Default::default()
                };
                if is_hit_line && let Some((s, e)) = hit_span {
                    let s = byte_floor(l, s);
                    let e = byte_floor(l, e).max(s);
                    let mut hi = fmt.clone();
                    hi.background = th.accent.to_color32().linear_multiply(0.5);
                    hi.color = Color32::WHITE;
                    job.append(&l[..s], 0.0, fmt.clone());
                    job.append(&l[s..e], 0.0, hi);
                    job.append(&l[e..], 0.0, fmt);
                } else {
                    job.append(l, 0.0, fmt);
                }
                job.wrap.max_width = row_w - 56.0;
                job.wrap.max_rows = 1;
                let galley = ui.fonts_mut(|f| f.layout_job(job));
                ui.painter().galley(
                    egui::pos2(rect.left() + 56.0, rect.center().y - galley.size().y * 0.5),
                    galley,
                    th.text.to_color32(),
                );
            }
        });
}

fn byte_floor(s: &str, mut idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn status_line(
    total: usize,
    truncated: bool,
    running: bool,
    error: Option<&str>,
    empty_query: bool,
) -> String {
    if let Some(e) = error {
        return format!("Error: {e}");
    }
    if empty_query {
        return String::from(" ");
    }
    let count = if truncated {
        format!("{total}+ matches")
    } else {
        format!("{total} matches")
    };
    if running {
        format!("{count} — searching…")
    } else {
        count
    }
}

fn collect_roots(app: &App) -> SearchRoots {
    let active_project_path = app
        .active
        .as_ref()
        .and_then(|(pid, _, _)| app.projects.iter().find(|p| &p.id == pid))
        .map(|p| p.path.clone());
    let active_workspace_path = app.active_workspace_path().map(|p| p.to_path_buf());
    let all_project_paths: Vec<PathBuf> = app
        .projects
        .iter()
        .filter(|p| !p.missing)
        .map(|p| p.path.clone())
        .collect();
    SearchRoots {
        all_project_paths,
        active_project_path,
        active_workspace_path,
    }
}

struct SearchRoots {
    all_project_paths: Vec<PathBuf>,
    active_project_path: Option<PathBuf>,
    active_workspace_path: Option<PathBuf>,
}

impl SearchRoots {
    fn pick(&self, scope: SearchScope) -> Vec<PathBuf> {
        match scope {
            SearchScope::AllProjects => self.all_project_paths.clone(),
            SearchScope::ActiveProject => {
                self.active_project_path.iter().cloned().collect()
            }
            SearchScope::ActiveWorkspace => {
                self.active_workspace_path.iter().cloned().collect()
            }
        }
    }
}

fn spawn_search(
    state: &mut FindInFilesState,
    roots: &SearchRoots,
    ctx: egui::Context,
) {
    if let Some(flag) = state.cancel_flag.take() {
        flag.store(true, Ordering::Relaxed);
    }
    state.search_token = state.search_token.wrapping_add(1);
    let token = state.search_token;
    let results = state.results.clone();
    {
        let mut g = results.lock();
        *g = SearchResults::new(token);
    }
    if state.query.trim().is_empty() {
        return;
    }
    let cancel = Arc::new(AtomicBool::new(false));
    state.cancel_flag = Some(cancel.clone());

    let paths = roots.pick(state.scope);
    if paths.is_empty() {
        let mut g = results.lock();
        g.error = Some("No search roots available".into());
        return;
    }
    {
        let mut g = results.lock();
        g.running = true;
    }

    let query = state.query.clone();
    let case_sensitive = state.case_sensitive;
    let whole_word = state.whole_word;
    let regex = state.regex;
    let mask = state.file_mask.clone();

    std::thread::spawn(move || {
        run_search(
            &query,
            case_sensitive,
            whole_word,
            regex,
            &mask,
            &paths,
            results,
            token,
            cancel,
            ctx,
        );
    });
}

fn run_search(
    query: &str,
    case_sensitive: bool,
    whole_word: bool,
    regex: bool,
    mask: &str,
    paths: &[PathBuf],
    results: Arc<Mutex<SearchResults>>,
    token: u64,
    cancel: Arc<AtomicBool>,
    ctx: egui::Context,
) {
    // Build matcher. When `regex` is off, escape so the query is
    // treated as a literal.
    let pattern = if regex {
        query.to_string()
    } else {
        escape_regex(query)
    };
    let mut mb = RegexMatcherBuilder::new();
    mb.case_insensitive(!case_sensitive)
        .case_smart(!case_sensitive)
        .word(whole_word);
    let matcher = match mb.build(&pattern) {
        Ok(m) => m,
        Err(e) => {
            let mut g = results.lock();
            if g.token == token {
                g.running = false;
                g.error = Some(format!("Bad pattern: {e}"));
            }
            ctx.request_repaint();
            return;
        }
    };

    // Build the gitignore-aware walker. One walker per root path.
    let mut globs = Vec::new();
    for m in mask.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
        globs.push(m.to_string());
    }

    let last_repaint = Arc::new(Mutex::new(Instant::now()));

    for root in paths {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let mut wb = WalkBuilder::new(root);
        wb.hidden(true) // skip hidden dirs/files (e.g. .git)
            .git_ignore(true)
            .git_exclude(true)
            .git_global(true)
            .ignore(true)
            .parents(true)
            .follow_links(false);
        if !globs.is_empty() {
            let mut og = ignore::overrides::OverrideBuilder::new(root);
            for g in &globs {
                if og.add(g).is_err() {
                    let mut r = results.lock();
                    if r.token == token {
                        r.error = Some(format!("Bad glob: {g}"));
                    }
                    return;
                }
            }
            if let Ok(o) = og.build() {
                wb.overrides(o);
            }
        }
        let walker = wb.build();
        for dent in walker {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let dent = match dent {
                Ok(d) => d,
                Err(_) => continue,
            };
            if !dent.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let path = dent.path().to_path_buf();
            let display_path = display_for(&path, paths);
            let res = search_one_file(
                &path,
                &display_path,
                &matcher,
                &results,
                token,
                &cancel,
                &ctx,
                &last_repaint,
            );
            if matches!(res, SearchOutcome::Truncated | SearchOutcome::Cancelled) {
                return;
            }
        }
    }

    let mut g = results.lock();
    if g.token == token {
        g.running = false;
    }
    ctx.request_repaint();
}

enum SearchOutcome {
    Continue,
    Truncated,
    Cancelled,
}

struct CollectSink<'a> {
    path: &'a Path,
    display_path: &'a str,
    results: &'a Arc<Mutex<SearchResults>>,
    matcher: &'a grep_regex::RegexMatcher,
    token: u64,
    cancel: &'a Arc<AtomicBool>,
    ctx: &'a egui::Context,
    last_repaint: &'a Arc<Mutex<Instant>>,
    truncated: bool,
    cancelled: bool,
}

impl<'a> Sink for CollectSink<'a> {
    type Error = std::io::Error;

    fn matched(
        &mut self,
        _searcher: &Searcher,
        m: &SinkMatch<'_>,
    ) -> Result<bool, Self::Error> {
        if self.cancel.load(Ordering::Relaxed) {
            self.cancelled = true;
            return Ok(false);
        }
        let bytes = m.bytes();
        let line_text = String::from_utf8_lossy(bytes).into_owned();
        let line_no = m.line_number().unwrap_or(0) as u32;
        // Find every submatch on this line.
        let mut spans: Vec<(usize, usize)> = Vec::new();
        let _ = self.matcher.find_iter(bytes, |mm| {
            spans.push((mm.start(), mm.end()));
            true
        });
        if spans.is_empty() {
            spans.push((0, bytes.len().min(1)));
        }
        for (s, e) in spans {
            let mut g = self.results.lock();
            if g.token != self.token {
                self.cancelled = true;
                return Ok(false);
            }
            if g.matches.len() >= MAX_RESULTS {
                g.truncated = true;
                self.truncated = true;
                return Ok(false);
            }
            g.files_seen.insert(self.path.to_path_buf());
            g.matches.push(SearchMatch {
                path: self.path.to_path_buf(),
                display_path: self.display_path.to_string(),
                line: line_no,
                byte_start: s,
                byte_end: e,
                line_text: line_text.clone(),
            });
        }
        // Repaint at most 20Hz so big result sets don't flood egui.
        let mut last = self.last_repaint.lock();
        if last.elapsed() >= Duration::from_millis(50) {
            self.ctx.request_repaint();
            *last = Instant::now();
        }
        Ok(true)
    }
}

fn search_one_file(
    path: &Path,
    display_path: &str,
    matcher: &grep_regex::RegexMatcher,
    results: &Arc<Mutex<SearchResults>>,
    token: u64,
    cancel: &Arc<AtomicBool>,
    ctx: &egui::Context,
    last_repaint: &Arc<Mutex<Instant>>,
) -> SearchOutcome {
    let mut searcher = Searcher::new();
    let mut sink = CollectSink {
        path,
        display_path,
        results,
        matcher,
        token,
        cancel,
        ctx,
        last_repaint,
        truncated: false,
        cancelled: false,
    };
    let _ = searcher.search_path(matcher, path, &mut sink);
    if sink.truncated {
        return SearchOutcome::Truncated;
    }
    if sink.cancelled {
        return SearchOutcome::Cancelled;
    }
    SearchOutcome::Continue
}

fn escape_regex(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' | '.' | '+' | '*' | '?' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^'
            | '$' | '#' | '&' | '-' | '~' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

fn display_for(path: &Path, roots: &[PathBuf]) -> String {
    for r in roots {
        if let Ok(rel) = path.strip_prefix(r) {
            let root_name = r
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if root_name.is_empty() {
                return rel.to_string_lossy().to_string();
            }
            return format!("{root_name}/{}", rel.to_string_lossy());
        }
    }
    path.to_string_lossy().to_string()
}
