// PATCH: Integrated cuda_scaling_method and auto_hw_decoder logic

use crate::cuda_scaling_method::apply_cuda_scaling_method;
use crate::auto_hw_decoder::auto_select_decoder;

fn build_cuda_filters(user_filter: &str, scaling_method: &str) -> String {
    let mut filter_chain = user_filter.to_string();
    if filter_chain.contains("scale=") {
        filter_chain = filter_chain.replace("scale=", &apply_cuda_scaling_method(scaling_method));
    }
    filter_chain
}

// Inside Encode::to_ffmpeg_args
let scaling_method = self.cuda_scaling_method.as_deref().unwrap_or("lanczos");
let cuda_filters = build_cuda_filters(&self.vfilter, scaling_method);

if self.auto_hw_decoder {
    if let Some(codec) = detected_input_codec() {
        if let Some(dec) = auto_select_decoder(&codec) {
            cuda_input_args.push("-hwaccel".into());
            cuda_input_args.push("cuda".into());
            cuda_input_args.push("-hwaccel_output_format".into());
            cuda_input_args.push("cuda".into());
            cuda_input_args.push("-c:v".into());
            cuda_input_args.push(dec.into());
        }
    }
}
