//! vmaf logic
use crate::process::{cmd_err, exit_ok_stderr, Chunks, CommandExt, FfmpegOut};
use anyhow::Context;
use log::{debug, info};
use std::{path::Path, process::Stdio};
use tokio::process::Command;
use tokio_process_stream::{Item, ProcessChunkStream};
use tokio_stream::{Stream, StreamExt};

/// Calculate VMAF score by converting the original first to yuv.
/// This can produce more accurate results than testing directly from original source.
pub fn run(
    reference: &Path,
    distorted: &Path,
    filter_complex: &str,
) -> anyhow::Result<impl Stream<Item = VmafOut>> {
    info!(
        "vmaf {} vs reference {}",
        distorted.file_name().and_then(|n| n.to_str()).unwrap_or(""),
        reference.file_name().and_then(|n| n.to_str()).unwrap_or(""),
    );

    let mut cmd = Command::new("ffmpeg");
    cmd.kill_on_drop(true)
        .arg2("-i", distorted)
        .arg2("-i", reference)
        .arg2("-filter_complex", filter_complex)
        .arg2("-f", "null")
        .arg("-")
        .stdin(Stdio::null());

    let cmd_str = cmd.to_cmd_str();
    debug!("cmd `{cmd_str}`");
    let vmaf: ProcessChunkStream = cmd.try_into().context("ffmpeg vmaf")?;

    Ok(async_stream::stream! {
        let mut vmaf = vmaf;
        let mut chunks = Chunks::default();
        let mut parsed_done = false;
        while let Some(next) = vmaf.next().await {
            match next {
                Item::Stderr(chunk) => {
                    if let Some(out) = VmafOut::try_from_chunk(&chunk, &mut chunks) {
                        if matches!(out, VmafOut::Done(_)) {
                            parsed_done = true;
                        }
                        yield out;
                    }
                }
                Item::Stdout(_) => {}
                Item::Done(code) => {
                    if let Err(err) = exit_ok_stderr("ffmpeg vmaf", code, &cmd_str, &chunks) {
                        yield VmafOut::Err(err);
                    }
                }
            }
        }
        if !parsed_done {
            yield VmafOut::Err(cmd_err(
                "could not parse ffmpeg vmaf score",
                &cmd_str,
                &chunks,
            ));
        }
    })
}

#[derive(Debug)]
pub enum VmafOut {
    Progress(FfmpegOut),
    Done(f32),
    Err(anyhow::Error),
}

impl VmafOut {
    fn try_from_chunk(chunk: &[u8], chunks: &mut Chunks) -> Option<Self> {
        const VMAF_SCORE_PRE: &str = "VMAF score: ";

        chunks.push(chunk);

        if let Some(line) = chunks.rfind_line(|l| l.contains(VMAF_SCORE_PRE)) {
            let idx = line.find(VMAF_SCORE_PRE).unwrap();
            return Some(Self::Done(
                line[idx + VMAF_SCORE_PRE.len()..].trim().parse().ok()?,
            ));
        }
        if let Some(progress) = FfmpegOut::try_parse(chunks.last_line()) {
            return Some(Self::Progress(progress));
        }
        None
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn parse_vmaf_score_207() {
        const FFMPEG_OUT: &str = r#"ffmpeg version n7.0.1 Copyright (c) 2000-2024 the FFmpeg developers
  built with gcc 14.1.1 (GCC) 20240522
  configuration: --prefix=/usr --disable-debug --disable-static --disable-stripping --enable-amf --enable-avisynth --enable-cuda-llvm --enable-lto --enable-fontconfig --enable-frei0r --enable-gmp --enable-gpl --enable-ladspa --enable-libaom --enable-libass --enable-libbluray --enable-libbs2b --enable-libdav1d --enable-libdrm --enable-libdvdnav --enable-libdvdread --enable-libfreetype --enable-libfribidi --enable-libgsm --enable-libharfbuzz --enable-libiec61883 --enable-libjack --enable-libjxl --enable-libmodplug --enable-libmp3lame --enable-libopencore_amrnb --enable-libopencore_amrwb --enable-libopenjpeg --enable-libopenmpt --enable-libopus --enable-libplacebo --enable-libpulse --enable-librav1e --enable-librsvg --enable-librubberband --enable-libsnappy --enable-libsoxr --enable-libspeex --enable-libsrt --enable-libssh --enable-libsvtav1 --enable-libtheora --enable-libv4l2 --enable-libvidstab --enable-libvmaf --enable-libvorbis --enable-libvpl --enable-libvpx --enable-libwebp --enable-libx264 --enable-libx265 --enable-libxcb --enable-libxml2 --enable-libxvid --enable-libzimg --enable-mbedtls --enable-nvdec --enable-nvenc --enable-opencl --enable-opengl --enable-shared --enable-vapoursynth --enable-version3 --enable-vulkan
  libavutil      59.  8.100 / 59.  8.100
  libavcodec     61.  3.100 / 61.  3.100
  libavformat    61.  1.100 / 61.  1.100
  libavdevice    61.  1.100 / 61.  1.100
  libavfilter    10.  1.100 / 10.  1.100
  libswscale      8.  1.100 /  8.  1.100
  libswresample   5.  1.100 /  5.  1.100
  libpostproc    58.  1.100 / 58.  1.100

  libavutil      59.  8.100 / 59.  8.100
  libavcodec     61.  3.100 / 61.  3.100
  libavformat    61.  1.100 / 61.  1.100
  libavdevice    61.  1.100 / 61.  1.100
  libavfilter    10.  1.100 / 10.  1.100
  libswscale      8.  1.100 /  8.  1.100
  libswresample   5.  1.100 /  5.  1.100
  libpostproc    58.  1.100 / 58.  1.100
Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'C:\Users\Administrator\Personal_scripts\Python\PythonScripts\PythonScripts\src\.ab-av1-RM46M2PZOVjb\A11 崩三 黑曼巴之影_1.sample2+600f.av1.crf37.5.mp4':
  Metadata:
    major_brand     : isom
    minor_version   : 512
    compatible_brands: isomav01iso2mp41
    title           : Project 1
    date            : 2019-07-11
    encoder         : Lavf61.1.100
  Duration: 00:00:20.00, start: 0.000000, bitrate: 1562 kb/s
  Stream #0:0[0x1](und): Video: av1 (libdav1d) (Main) (av01 / 0x31307661), yuv420p10le(tv, progressive), 1000x696, 1560 kb/s, SAR 1:1 DAR 125:87, 30 fps, 30 tbr, 15360 tbn (default)
      Metadata:
        handler_name    : VideoHandler
        vendor_id       : [0][0][0][0]
        encoder         : Lavc61.3.100 libsvtav1
Input #1, matroska,webm, from 'C:\Users\Administrator\Personal_scripts\Python\PythonScripts\PythonScripts\src\.ab-av1-RM46M2PZOVjb\A11 崩三 黑曼巴之影_1.sample2+600f.mkv':
  Metadata:
    title           : Project 1
    DATE            : 2019-07-11
    MAJOR_BRAND     : isom
    MINOR_VERSION   : 512
    COMPATIBLE_BRANDS: isomiso2mp41
    ENCODER         : Lavf61.1.100
  Duration: 00:00:20.00, start: 0.000000, bitrate: 6114 kb/s
  Stream #1:0: Video: mpeg4 (Simple Profile), yuv420p, 1000x696 [SAR 1:1 DAR 125:87], 30 fps, 30 tbr, 1k tbn (default)
      Metadata:
        HANDLER_NAME    : VideoHandler
        VENDOR_ID       : [0][0][0][0]
        DURATION        : 00:00:20.000000000
Stream mapping:
  Stream #0:0 (libdav1d) -> format:default
  Stream #1:0 (mpeg4) -> format:default
  libvmaf:default -> Stream #0:0 (wrapped_avframe)
Press [q] to stop, [?] for help
Output #0, null, to 'pipe:':
  Metadata:
    major_brand     : isom
    minor_version   : 512
    compatible_brands: isomav01iso2mp41
    title           : Project 1
    date            : 2019-07-11
    encoder         : Lavf61.1.100
  Stream #0:0: Video: wrapped_avframe, yuv420p10le(tv, progressive), 1552x1080 [SAR 5625:5626 DAR 125:87], q=2-31, 200 kb/s, 24 tbn
      Metadata:
        encoder         : Lavc61.3.100 wrapped_avframe
frame=   48 fps=0.0 q=-0.0 size=N/A time=00:00:01.95 bitrate=N/A speed=3.79x    
frame=  101 fps= 97 q=-0.0 size=N/A time=00:00:04.16 bitrate=N/A speed=   4x    
frame=  156 fps=100 q=-0.0 size=N/A time=00:00:06.45 bitrate=N/A speed=4.14x    
frame=  209 fps=101 q=-0.0 size=N/A time=00:00:08.66 bitrate=N/A speed= 4.2x    
frame=  264 fps=102 q=-0.0 size=N/A time=00:00:10.95 bitrate=N/A speed=4.23x    
frame=  319 fps=103 q=-0.0 size=N/A time=00:00:13.25 bitrate=N/A speed=4.26x    
frame=  373 fps=103 q=-0.0 size=N/A time=00:00:15.50 bitrate=N/A speed=4.27x    
frame=  429 fps=103 q=-0.0 size=N/A time=00:00:17.83 bitrate=N/A speed= 4.3x    
frame=  482 fps=103 q=-0.0 size=N/A time=00:00:20.04 bitrate=N/A speed=4.29x    
frame=  536 fps=104 q=-0.0 size=N/A time=00:00:22.29 bitrate=N/A speed=4.31x    
frame=  589 fps=103 q=-0.0 size=N/A time=00:00:24.50 bitrate=N/A speed= 4.3x    
[Parsed_libvmaf_6 @ 000002b296bac480] VMAF score: 94.826380
[out#0/null @ 000002b2916f8b80] video:258KiB audio:0KiB subtitle:0KiB other streams:0KiB global headers:0KiB muxing overhead: unknown
frame=  600 fps=102 q=-0.0 Lsize=N/A time=00:00:24.95 bitrate=N/A speed=4.24x"#;

        const CHUNK_SIZE: usize = 64;

        let ffmpeg = FFMPEG_OUT.as_bytes();

        let mut chunks = Chunks::default();
        let mut start_idx = 0;
        let mut vmaf_score = None;
        while start_idx < ffmpeg.len() {
            let chunk = &ffmpeg[start_idx..(start_idx + CHUNK_SIZE).min(FFMPEG_OUT.len())];
            println!("* {}", String::from_utf8_lossy(chunk).trim());

            if let Some(vmaf) = VmafOut::try_from_chunk(chunk, &mut chunks) {
                println!("{vmaf:?}");
                if let VmafOut::Done(score) = vmaf {
                    vmaf_score = Some(score);
                }
            }

            start_idx += CHUNK_SIZE;
        }

        assert_eq!(vmaf_score, Some(94.82638), "failed to parse vmaf score");
    }
}
