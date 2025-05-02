// src/cuda.rs
#[derive(Clone, Debug)]
pub struct CudaConfig {
    pub decoder: String,
    pub filters: Vec<String>,
    pub surfaces: usize,
}

impl CudaConfig {
    pub fn ffmpeg_args(&self) -> Vec<String> {
        let mut args = vec![
            "-hwaccel".into(),
            "cuda".into(),
            "-hwaccel_output_format".into(),
            "cuda".into(),
            "-extra_hw_frames".into(),
            format!("{}", self.surfaces),
            "-c:v".into(),
            self.decoder.clone(),
        ];
        
        if !self.filters.is_empty() {
            args.push("-vf".into());
            args.push(self.filters.join(","));
        }
        
        args
    }
}
