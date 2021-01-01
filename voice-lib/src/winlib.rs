use winapi::um::{errhandlingapi, processthreadsapi, avrt};
use std::os::raw::c_int;
use winapi::um::winbase::THREAD_PRIORITY_TIME_CRITICAL;

#[cfg(windows)]
pub fn bump_priority() {
    unsafe {
        let result = processthreadsapi::SetThreadPriority(
            processthreadsapi::GetCurrentThread(),
            THREAD_PRIORITY_TIME_CRITICAL as c_int,
        );
        if result == 0 {
            println!("Failed to set thread priority: {}", errhandlingapi::GetLastError());
        }
        let task_name = "Pro Audio\0".to_string();
        let mut task_index = 0;
        let handle = avrt::AvSetMmThreadCharacteristicsA(task_name.as_ptr() as _, &mut task_index);
        if handle.is_null() {
            eprintln!("Unable to bump thread priority using mmcss");
        }
    }
}
