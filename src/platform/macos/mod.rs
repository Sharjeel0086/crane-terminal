use std::path::Path;
use std::process::Command;
use egui::ScrollArea;

pub fn open_externally(path: &Path) {
    if let Err(e) = Command::new("open").arg(path).spawn() {
        log::warn!("open externally failed: {e}");
    }
}

pub fn reveal_in_file_manager(path: &str) {
    let _ = Command::new("open").arg("-R").arg(path).spawn();
}

pub fn reveal_label() -> &'static str {
    "Reveal in Finder"
}

pub fn handle_paste_event(
    _i: &mut egui::InputState,
    _ctrl_v: bool,
    _pt: &mut Option<String>,
    pi: &mut Option<std::path::PathBuf>,
) {
    let pending = crate::mac_keys::drain_pending_image_paths();
    if let Some(path) = pending.into_iter().next() {
        *pi = Some(std::path::PathBuf::from(path));
    }
}

pub fn render_image_preview(
    ui: &mut egui::Ui,
    active_idx: usize,
    editor_h: f32,
    path: &str,
    texture: &mut Option<egui::TextureHandle>,
) {
    if texture.is_none() {
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(img) = image::load_from_memory(&bytes) {
                let rgba = img.to_rgba8();
                let size = [rgba.width() as usize, rgba.height() as usize];
                let color = egui::ColorImage::from_rgba_unmultiplied(size, &rgba);
                *texture = Some(ui.ctx().load_texture(
                    format!("crane_img:{}", path),
                    color,
                    egui::TextureOptions::LINEAR,
                ));
            }
        }
    }
    ScrollArea::both()
        .id_salt(("image_scroll", active_idx))
        .auto_shrink([false; 2])
        .max_height(editor_h)
        .show(ui, |ui| {
            if let Some(tex) = texture {
                let size = tex.size_vec2();
                ui.add(egui::Image::from_texture(tex).fit_to_original_size(1.0).max_size(size));
            } else {
                ui.label(
                    egui::RichText::new("Couldn't decode image")
                        .color(crate::theme::current().error.to_color32()),
                );
            }
pub fn system_fallback_fonts() -> &'static [(&'static str, &'static str, u32)] {
    &[
        ("pingfang", "/System/Library/Fonts/PingFang.ttc", 0),
        ("hiragino_sans_gb", "/System/Library/Fonts/Hiragino Sans GB.ttc", 0),
        ("apple_sd_gothic", "/System/Library/Fonts/AppleSDGothicNeo.ttc", 0),
        ("arial_hb", "/System/Library/Fonts/Supplemental/ArialHB.ttc", 0),
        ("geeza_pro", "/System/Library/Fonts/Supplemental/GeezaPro.ttc", 0),
        ("kohinoor", "/System/Library/Fonts/Kohinoor.ttc", 0),
        ("apple_symbols", "/System/Library/Fonts/Apple Symbols.ttf", 0),
    ]
}

pub fn init_app() {}

pub fn fix_path_for_gui_launch() {
    let original = std::env::var("PATH").unwrap_or_default();
    let looks_gui = !original.contains("/usr/local/bin")
        && !original.contains("/opt/homebrew/bin")
        && !original.contains(".cargo/bin")
        && std::env::var("HOME").is_ok();

    let mut current = original.clone();
    if looks_gui {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let output = std::process::Command::new(&shell)
            .arg("-l")
            .arg("-c")
            .arg("echo __CRANE_PATH__:$PATH")
            .output();
        if let Ok(out) = output {
            let s = String::from_utf8_lossy(&out.stdout);
            if let Some(line) = s.lines().find(|l| l.starts_with("__CRANE_PATH__:")) {
                let path = line.trim_start_matches("__CRANE_PATH__:").to_string();
                if !path.is_empty() {
                    current = path;
                }
            }
        }
    }

    let home = std::env::var("HOME").unwrap_or_default();
    let mut extras: Vec<String> = vec![
        format!("{home}/.cargo/bin"),
        format!("{home}/.local/bin"),
        format!("{home}/bin"),
        format!("{home}/go/bin"),
        format!("{home}/.volta/bin"),
        format!("{home}/.fnm/aliases/default/bin"),
        format!("{home}/.asdf/shims"),
        format!("{home}/.bun/bin"),
        format!("{home}/n/bin"),
        "/opt/homebrew/bin".to_string(),
        "/opt/homebrew/sbin".to_string(),
        "/usr/local/bin".to_string(),
    ];
    let nvm_dir = std::path::PathBuf::from(format!("{home}/.nvm/versions/node"));
    if let Ok(rd) = std::fs::read_dir(&nvm_dir) {
        let mut versions: Vec<(std::time::SystemTime, String)> = rd
            .flatten()
            .filter_map(|e| {
                let p = e.path().join("bin");
                if !p.is_dir() {
                    return None;
                }
                let mtime = e
                    .metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                Some((mtime, p.to_string_lossy().into_owned()))
            })
            .collect();
        versions.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, p) in versions {
            extras.push(p);
        }
    }

    let mut seen = std::collections::HashSet::new();
    let mut parts: Vec<String> = Vec::new();
    for p in extras.into_iter().chain(current.split(':').map(|s| s.to_string())) {
        if p.is_empty() || !seen.insert(p.clone()) {
            continue;
        }
        parts.push(p);
    }
    unsafe { std::env::set_var("PATH", parts.join(":")) };
}

pub fn restore_trash_item(_path: &std::path::Path) -> Result<(), String> {
    Err("open Finder → Trash → right-click → Put Back".into())
}

pub fn has_foreground_process(shell_pid: Option<u32>, master: &(dyn portable_pty::MasterPty + Send)) -> bool {
    use std::os::unix::io::AsRawFd;
    let Some(shell) = shell_pid else { return false; };
    let Some(fd) = master.as_raw_fd() else { return false; };
    let fg = unsafe { libc::tcgetpgrp(fd) };
    if fg < 0 { return false; }
    (fg as u32) != shell
}

pub fn foreground_process_name(shell_pid: Option<u32>, master: &(dyn portable_pty::MasterPty + Send)) -> Option<String> {
    use std::os::unix::io::AsRawFd;
    let shell = shell_pid?;
    let fd = master.as_raw_fd()?;
    let fg = unsafe { libc::tcgetpgrp(fd) };
    if fg < 0 || (fg as u32) == shell { return None; }
    let out = std::process::Command::new("ps")
        .args(["-o", "comm=", "-p"])
        .arg(fg.to_string())
        .output()
        .ok()?;
    if !out.status.success() { return None; }
    let raw = String::from_utf8(out.stdout).ok()?;
    let name = raw.trim();
    if name.is_empty() { return None; }
    let basename = name.rsplit('/').next().unwrap_or(name);
    Some(basename.to_string())
}



pub fn canonicalize_path(path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    std::fs::canonicalize(path)
}
