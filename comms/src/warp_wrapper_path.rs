use std::path::PathBuf;

pub fn get_warp_wrapper_path() -> Option<PathBuf> {
    std::env::var_os("WARP_WRAPPER_PATH").map(PathBuf::from)
}
