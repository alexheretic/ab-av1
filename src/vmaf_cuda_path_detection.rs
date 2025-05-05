// Looks up vmaf_cuda from PATH or env var
use std::env;
pub fn find_vmaf_cuda() -> String {
    env::var("VMAF_CUDA_PATH").unwrap_or_else(|_| "vmaf_cuda".to_string())
}
