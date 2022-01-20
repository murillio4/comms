use winapi::um::avrt;
use windows::Win32::System::Threading;

#[cfg(windows)]
pub fn bump_priority() {
    unsafe {
        if let Err(e) = Threading::SetThreadPriority(
            Threading::GetCurrentThread(),
            Threading::THREAD_PRIORITY_TIME_CRITICAL,
        )
        .ok()
        {
            eprintln!("Failed to set thread priority: {}", e);
        }
        let task_name = "Pro Audio\0".to_string();
        let mut task_index = 0;
        let handle = avrt::AvSetMmThreadCharacteristicsA(task_name.as_ptr() as _, &mut task_index);
        if handle.is_null() {
            eprintln!("Unable to bump thread priority using mmcss");
        }
    }
}
