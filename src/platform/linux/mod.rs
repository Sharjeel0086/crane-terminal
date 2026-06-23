use std::path::Path;
use std::process::Command;
use egui::ScrollArea;

pub fn open_externally(path: &Path) {
    if let Err(e) = Command::new("xdg-open").arg(path).spawn() {
        log::warn!("open externally failed: {e}");
    }
}

pub fn reveal_in_file_manager(path: &str) {
    let parent = Path::new(path).parent().unwrap_or_else(|| Path::new("/"));
    let _ = Command::new("xdg-open").arg(parent).spawn();
}

pub fn reveal_label() -> &'static str {
    "Reveal in Files"
}

pub fn handle_paste_event(
    i: &mut egui::InputState,
    ctrl_v: bool,
    pt: &mut Option<String>,
    pi: &mut Option<std::path::PathBuf>,
) {
    if ctrl_v {
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            if let Ok(text) = clipboard.get_text() {
                *pt = Some(text);
            }
        }
        if pt.is_none() {
            *pi = get_clipboard_image();
        }
        i.consume_key(egui::Modifiers::CTRL, egui::Key::V);
        i.consume_key(egui::Modifiers::COMMAND, egui::Key::V);
        i.consume_key(egui::Modifiers::SHIFT, egui::Key::Insert);
        i.events.retain(|e| !matches!(e, egui::Event::Text(_) | egui::Event::Paste(_)));
    }
}

pub fn get_clipboard_image() -> Option<std::path::PathBuf> {
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        if let Ok(img_data) = clipboard.get_image() {
            if let Some(img) = image::RgbaImage::from_raw(
                img_data.width as u32,
                img_data.height as u32,
                img_data.bytes.into_owned(),
            ) {
                let tmp_dir = match crate::util::home_dir() {
                    Some(h) => h.join(".crane").join("tmp_images"),
                    None => std::env::temp_dir().join("crane").join("tmp_images"),
                };
                let _ = std::fs::create_dir_all(&tmp_dir);
                let timestamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis();
                let path = tmp_dir.join(format!("pasted_image_{}.png", timestamp));

                let dyn_img = image::DynamicImage::ImageRgba8(img);
                if dyn_img.save(&path).is_ok() {
                    return Some(path);
                }
            }
        }
    }
    None
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
        ("noto_cjk", "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0),
        ("noto_cjk", "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 0),
        ("noto_cjk", "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc", 0),
        ("noto_cjk", "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc", 0),
        ("wqy_zenhei", "/usr/share/fonts/wenquanyi/wqy-zenhei.ttc", 0),
        ("noto_sans_arabic", "/usr/share/fonts/truetype/noto/NotoSansArabic-Regular.ttf", 0),
        ("noto_sans_hebrew", "/usr/share/fonts/truetype/noto/NotoSansHebrew-Regular.ttf", 0),
        ("noto_sans_devanagari", "/usr/share/fonts/truetype/noto/NotoSansDevanagari-Regular.ttf", 0),
    ]
}

pub fn init_app() {
    if let Err(e) = gtk::init() {
        eprintln!(
            "[crane] gtk::init failed: {e}. Browser pane will be unavailable. \
             Install libgtk-3 + libwebkit2gtk-4.1 and relaunch."
        );
    }
}

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

pub fn restore_trash_item(path: &std::path::Path) -> Result<(), String> {
    use trash::os_limited;
    let parent = path.parent().ok_or("no parent dir")?.to_path_buf();
    let name = path.file_name().ok_or("no file name")?.to_os_string();
    let items = os_limited::list().map_err(|e| format!("list: {e}"))?;
    let target = items.into_iter().find(|it| it.original_parent == parent && it.name == name);
    match target {
        Some(item) => {
            os_limited::restore_all([item]).map_err(|e| e.to_string())?;
            Ok(())
        }
        None => Err("not found in trash (already restored or emptied?)".into())
    }
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
