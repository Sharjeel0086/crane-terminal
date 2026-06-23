use std::path::Path;
use std::process::Command;
use egui::ScrollArea;

pub fn open_externally(path: &Path) {
    if let Err(e) = Command::new("explorer").arg(path).spawn() {
        log::warn!("open externally failed: {e}");
    }
}

pub fn reveal_in_file_manager(path: &str) {
    let _ = Command::new("explorer").arg(format!("/select,{}", path)).spawn();
}

pub fn reveal_label() -> &'static str {
    "Reveal in Explorer"
}

pub fn handle_paste_event(
    i: &mut egui::InputState,
    mut ctrl_v: bool,
    pt: &mut Option<String>,
    pi: &mut Option<std::path::PathBuf>,
) {
    if !ctrl_v {
        // Fallback: Check global keyboard state because egui_winit swallows Ctrl+V 
        // when clipboard.get_text() fails, meaning egui never sees the key press.
        let ctrl = i.modifiers.ctrl || i.modifiers.command;
        let shift = i.modifiers.shift;
        unsafe {
            let v_state = winapi::um::winuser::GetAsyncKeyState(0x56) as u16;
            // Check MSB (currently down) OR LSB (was tapped since last call)
            let v = (v_state & 0x8000) != 0 || (v_state & 1) != 0;
            
            let insert_state = winapi::um::winuser::GetAsyncKeyState(0x2D) as u16;
            let insert = (insert_state & 0x8000) != 0 || (insert_state & 1) != 0;
            
            if (ctrl && v) || (shift && insert) {
                static mut LAST_PASTE: Option<std::time::Instant> = None;
                let now = std::time::Instant::now();
                if LAST_PASTE.map(|t| now.duration_since(t).as_millis() > 300).unwrap_or(true) {
                    let fg = winapi::um::winuser::GetForegroundWindow();
                    let mut pid: u32 = 0;
                    winapi::um::winuser::GetWindowThreadProcessId(fg, &mut pid);
                    if pid == std::process::id() {
                        ctrl_v = true;
                        LAST_PASTE = Some(now);
                    }
                }
            }
        }
    }

    if ctrl_v {
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            if let Ok(text) = clipboard.get_text() {
                *pt = Some(text);
            }
        }
        if pt.is_none() {
            *pi = get_clipboard_image();
            if pi.is_some() {
                // Emulate the macOS OS-level privacy banner for clipboard access
                let _ = notify_rust::Notification::new()
                    .appname("Crane")
                    .summary("Image pasted successfully")
                    .body("The clipboard image was successfully pasted into Crane.")
                    .show();
            }
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
    _texture: &mut Option<egui::TextureHandle>,
) {
    ScrollArea::both()
        .id_salt(("image_scroll", active_idx))
        .auto_shrink([false; 2])
        .max_height(editor_h)
        .show(ui, |ui| {
            let bytes = std::fs::read(path).unwrap_or_default();
            let dimensions = if let Ok(img) = image::load_from_memory(&bytes) {
                format!("{} x {}", img.width(), img.height())
            } else {
                "Unknown dimensions".to_string()
            };
            let text = format!(
                "Read-only Image Preview\n\nPath: {}\nSize: {} bytes\nDimensions: {}",
                path,
                bytes.len(),
                dimensions
            );
            ui.label(egui::RichText::new(text).color(crate::theme::current().text.to_color32()));
        });
}

pub fn system_fallback_fonts() -> &'static [(&'static str, &'static str, u32)] {
    &[
        ("yahei", "C:\\Windows\\Fonts\\msyh.ttc", 0),
        ("yu_gothic", "C:\\Windows\\Fonts\\yugothm.ttc", 0),
        ("malgun_gothic", "C:\\Windows\\Fonts\\malgun.ttf", 0),
        ("tahoma", "C:\\Windows\\Fonts\\tahoma.ttf", 0),
        ("arial", "C:\\Windows\\Fonts\\arial.ttf", 0),
    ]
}

pub fn init_app() {
    // Many Windows environments have broken Vulkan drivers (e.g. from
    // older Intel integrated graphics or OBS Studio capture hooks) which
    // cause wgpu to crash with STATUS_ACCESS_VIOLATION deep in native code
    // when it probes Vulkan adapters. Force DirectX 12/11 to avoid this.
    if std::env::var("WGPU_BACKEND").is_err() {
        unsafe {
            std::env::set_var("WGPU_BACKEND", "dx12,dx11");
        }
    }
}

pub fn fix_path_for_gui_launch() {}

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

pub fn has_foreground_process(shell_pid: Option<u32>, _master: &(dyn portable_pty::MasterPty + Send)) -> bool {
    foreground_process_name(shell_pid, _master).is_some()
}

pub fn foreground_process_name(shell_pid: Option<u32>, _master: &(dyn portable_pty::MasterPty + Send)) -> Option<String> {
    let parent_pid = shell_pid?;
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
    
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            return None;
        }

        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            cntUsage: 0,
            th32ProcessID: 0,
            th32DefaultHeapID: 0,
            th32ModuleID: 0,
            cntThreads: 0,
            th32ParentProcessID: 0,
            pcPriClassBase: 0,
            dwFlags: 0,
            szExeFile: [0; 260],
        };

        let mut child_exe = None;

        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                if entry.th32ParentProcessID == parent_pid {
                    let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                    let exe_name = OsString::from_wide(&entry.szExeFile[..len]);
                    let name_str = exe_name.to_string_lossy().into_owned();
                    
                    let lower = name_str.to_lowercase();
                    // Ignore conhost.exe / OpenConsole.exe which might be spawned by conpty
                    if lower != "conhost.exe" && lower != "openconsole.exe" {
                        child_exe = Some(name_str);
                        break;
                    }
                }
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        windows_sys::Win32::Foundation::CloseHandle(snapshot);
        child_exe
    }
}

pub fn canonicalize_path(path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    dunce::canonicalize(path)
}

