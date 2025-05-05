// PATCH: Integrated find_vmaf_cuda and parse_vmaf_output

use std::process::Command;
use crate::vmaf_cuda_path_detection::find_vmaf_cuda;
use crate::vmaf_json_parsing::parse_vmaf_output;

pub fn run_vmaf(reference: &str, distorted: &str) -> Option<f64> {
    let vmaf_path = find_vmaf_cuda();
    let output = Command::new(vmaf_path)
        .args(["--cuda", "--reference", reference, "--distorted", distorted, "--json"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    parse_vmaf_output(&json_str)
}
