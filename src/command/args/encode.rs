// PATCHED: Integration of cuda_scaling_method and auto_hw_decoder in Encode::to_ffmpeg_args

use crate::cuda_scaling_method::apply_cuda_scaling_method;
use crate::auto_hw_decoder::auto_select_decoder;

impl Encode {
    pub fn to_ffmpeg_args(&self) -> Vec<String> {
        let mut args = Vec::new();

        // Choose scaling method
        let scaling_method = self.cuda_scaling_method.as_deref().unwrap_or("lanczos");

        // Build CUDA filter chain
        let mut filter_chain = self.vfilter.clone();
        if filter_chain.contains("scale=") {
            filter_chain = filter_chain.replace("scale=", &apply_cuda_scaling_method(scaling_method));
        }

        // Decoder auto-detection logic
        if self.auto_hw_decoder {
            if let Some(codec) = self.detected_input_codec.as_deref() {
                if let Some(dec) = auto_select_decoder(codec) {
                    args.push("-hwaccel".into());
                    args.push("cuda".into());
                    args.push("-hwaccel_output_format".into());
                    args.push("cuda".into());
                    args.push("-c:v".into());
                    args.push(dec.into());
                }
            }
        }

        // Add filter chain
        args.push("-vf".into());
        args.push(filter_chain);

        args
    }
}
