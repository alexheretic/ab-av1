use crate::auto_hw_decoder::auto_select_decoder;
use crate::cuda_scaling_method::apply_cuda_scaling_method;

pub struct Encode {
    pub vfilter: String,
    pub auto_hw_decoder: bool,
    pub cuda_scaling_method: Option<String>,
    pub detected_input_codec: Option<String>,
}

impl Encode {
    pub fn to_ffmpeg_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        let scaling_method = self.cuda_scaling_method.as_deref().unwrap_or("lanczos");

        // Filter logic
        let mut filter_chain = self.vfilter.clone();
        if filter_chain.contains("scale=") {
            filter_chain = filter_chain.replace("scale=", &apply_cuda_scaling_method(scaling_method));
        }

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

        args.push("-vf".into());
        args.push(filter_chain);
        args
    }
}
