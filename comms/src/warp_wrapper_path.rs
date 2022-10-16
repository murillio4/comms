use std::path::PathBuf;
use sysinfo::{ProcessExt, SystemExt};

pub fn get_warp_wrapper_path() -> Option<PathBuf> {
    let mut system = sysinfo::System::new();
    let pid = match sysinfo::get_current_pid() {
        Ok(pid) => pid,
        Err(_) => return None,
    };
    system.refresh_process_specifics(pid, sysinfo::ProcessRefreshKind::new());
    let current_process = match system.process(pid) {
        Some(process) => process,
        None => return None,
    };

    if !current_process.exe().to_string_lossy().contains("warp") {
        return None;
    }

    let parent_pid = match current_process.parent() {
        Some(pid) => pid,
        None => return None,
    };
    system.refresh_process_specifics(parent_pid, sysinfo::ProcessRefreshKind::new());
    let parent_process = match system.process(parent_pid) {
        Some(process) => process,
        None => return None,
    };

    return Some(parent_process.exe().to_owned());
}
