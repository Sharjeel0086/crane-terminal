use backtrace::Backtrace;
use std::fs::{self, File};
use std::io::Write;
use std::panic;
use std::path::PathBuf;

pub fn init() {
    panic::set_hook(Box::new(|info| {
        let backtrace = Backtrace::new();
        
        let msg = match info.payload().downcast_ref::<&'static str>() {
            Some(s) => *s,
            None => match info.payload().downcast_ref::<String>() {
                Some(s) => &s[..],
                None => "Box<dyn Any>",
            },
        };

        let location = info.location().unwrap();
        let thread = std::thread::current();
        let thread_name = thread.name().unwrap_or("<unnamed>");

        let crash_report = format!(
            "Crane Crash Report\n\
             ==========================\n\
             Thread: {}\n\
             Location: {}:{}\n\
             Message: {}\n\
             \n\
             Stack Trace:\n\
             {:?}\n",
            thread_name,
            location.file(),
            location.line(),
            msg,
            backtrace
        );

        eprintln!("{}", crash_report);

        if let Some(log_dir) = get_crash_log_dir() {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            let file_path = log_dir.join(format!("crane_crash_{}.log", timestamp));

            if let Ok(mut file) = File::create(&file_path) {
                let _ = file.write_all(crash_report.as_bytes());
            }
        }
    }));
}

fn get_crash_log_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
            let path = PathBuf::from(local_app_data).join("Crane").join("crash_logs");
            let _ = fs::create_dir_all(&path);
            return Some(path);
        }
    }
    
    // Fallback for non-windows or if LOCALAPPDATA is missing
    let dir = crate::util::home_dir()?.join(".crane").join("crash_logs");
    let _ = fs::create_dir_all(&dir);
    Some(dir)
}
