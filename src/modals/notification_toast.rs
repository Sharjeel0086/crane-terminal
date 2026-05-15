//! Bottom-right toast that surfaces OSC 9 / OSC 777 desktop
//! notifications emitted by programs running in a Crane terminal —
//! Claude Code's `Stop` / `Notification` hooks, build scripts that
//! `printf '\e]9;done\a'`, etc.
//!
//! Drains [`App::pending_notifications`] into `active_notification`
//! when nothing else is showing, runs a TTL clock, and on click
//! focuses the originating Pane. Urgent (OSC 777) toasts persist for
//! 12 s and get a coloured stroke; plain OSC 9 fades after 5 s.

use crate::state::{App, PaneNotification};
use std::time::Duration;

const NORMAL_TTL: Duration = Duration::from_secs(5);
const URGENT_TTL: Duration = Duration::from_secs(12);

/// Drive the toast: rotate next pending notification into the
/// foreground slot, expire stale ones, and paint the visible toast.
/// Cheap when nothing is queued and nothing active (single field
/// read + early return).
pub fn render(ctx: &egui::Context, app: &mut App) {
    if app.active_notification.is_none()
        && let Some(next) = app.pending_notifications.pop_front()
    {
        // Fire macOS Notification Center banner when the app isn't
        // focused — the in-app toast is for active users, the OS
        // banner is for users who switched away. Done before latching
        // `active_notification` so a rapid burst still produces one
        // banner per event.
        fire_os_notification(&next, app.window_focused);
        app.active_notification = Some(next);
    }

    // Time out the active toast if its TTL elapsed.
    let expired = app.active_notification.as_ref().map_or(false, |n| {
        let ttl = if n.urgent { URGENT_TTL } else { NORMAL_TTL };
        n.created_at.elapsed() >= ttl
    });
    if expired {
        app.active_notification = None;
    }

    let Some(notif) = app.active_notification.clone() else {
        return;
    };

    // Egui repaints on input; nothing in this toast triggers input
    // by itself, so without a manual repaint the TTL would never
    // fire on an idle frame. Schedule the next tick at the soonest
    // useful instant.
    let ttl = if notif.urgent { URGENT_TTL } else { NORMAL_TTL };
    let remaining = ttl.saturating_sub(notif.created_at.elapsed());
    ctx.request_repaint_after(remaining.min(Duration::from_millis(500)));

    let theme = crate::theme::current();
    let screen = ctx.content_rect();
    let toast_w = 420.0_f32.min(screen.width() - 40.0);
    let toast_h = 84.0_f32;
    let area_id = egui::Id::new("crane_pty_notification_toast");

    let stroke_color = if notif.urgent {
        theme.error.to_color32()
    } else {
        theme.border.to_color32()
    };

    egui::Area::new(area_id)
        .order(egui::Order::Tooltip)
        .fixed_pos(egui::pos2(
            screen.max.x - toast_w - 20.0,
            screen.max.y - toast_h - 28.0,
        ))
        .show(ctx, |ui| {
            egui::Frame::default()
                .fill(theme.surface.to_color32())
                .stroke(egui::Stroke::new(1.0, stroke_color))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(egui::Margin::same(12))
                .show(ui, |ui| {
                    ui.set_width(toast_w - 24.0);
                    let header_glyph = if notif.urgent {
                        egui_phosphor::regular::WARNING
                    } else {
                        egui_phosphor::regular::INFO
                    };
                    let header_color = if notif.urgent {
                        stroke_color
                    } else {
                        theme.accent.to_color32()
                    };
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(header_glyph)
                                .size(18.0)
                                .color(header_color),
                        );
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new("Terminal notification")
                                    .size(13.0)
                                    .color(theme.text.to_color32())
                                    .strong(),
                            );
                            let body = truncate_body(&notif.body, 180);
                            ui.label(
                                egui::RichText::new(body)
                                    .size(12.0)
                                    .color(theme.text_muted.to_color32()),
                            );
                        });
                        ui.with_layout(
                            egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                let close = ui.add(
                                    egui::Button::new(
                                        egui::RichText::new(egui_phosphor::regular::X)
                                            .size(13.0)
                                            .color(theme.text_muted.to_color32()),
                                    )
                                    .frame(false)
                                    .min_size(egui::vec2(22.0, 22.0)),
                                );
                                if close.hovered() {
                                    ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                                }
                                if close.clicked() {
                                    app.active_notification = None;
                                }
                            },
                        );
                    });

                    // Click-through on the body area focuses the
                    // originating Pane. We probe the full inner rect
                    // (minus the close-button column) so the click
                    // target is large.
                    let body_rect = ui.min_rect();
                    let resp = ui.interact(
                        body_rect,
                        area_id.with("body_click"),
                        egui::Sense::click(),
                    );
                    if resp.hovered() {
                        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if resp.clicked() {
                        app.focus_notification_source(&notif);
                        app.active_notification = None;
                    }
                });
        });
}

fn truncate_body(s: &str, max_chars: usize) -> String {
    let mut out: String = s.chars().take(max_chars).collect();
    if s.chars().count() > max_chars {
        out.push('\u{2026}');
    }
    out
}

/// Fire an OS-level notification (macOS Notification Center / Linux
/// libnotify / Windows toast) when the Crane window isn't focused.
/// In-app toast still always fires; this is the background-attention
/// path. Best-effort — failures are swallowed.
///
/// We use [`notify_rust`] rather than shelling out to `osascript`
/// because osascript attributes every notification it sends to
/// "Script Editor.app" regardless of the title text, which surfaces
/// as a confusing source in macOS Notification Center. `notify_rust`
/// on macOS uses NSUserNotificationCenter, which inherits the
/// calling process's bundle identity — so when Crane is launched
/// from its bundled `.app`, the notification source reads "Crane".
/// In dev (`cargo run`), it falls back to the spawning terminal's
/// bundle, which is acceptable.
fn fire_os_notification(n: &PaneNotification, window_focused: bool) {
    if window_focused {
        return;
    }
    let title = if n.urgent { "Crane — urgent" } else { "Crane" };
    let _ = notify_rust::Notification::new()
        .summary(title)
        .body(&n.body)
        .show();
}
