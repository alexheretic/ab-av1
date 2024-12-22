//! xpsnr logic
use crate::process::{cmd_err, exit_ok_stderr, Chunks, CommandExt, FfmpegOut};
use anyhow::Context;
use log::{debug, info};
use std::{path::Path, process::Stdio};
use tokio::process::Command;
use tokio_process_stream::{Item, ProcessChunkStream};
use tokio_stream::{Stream, StreamExt};

/// Calculate XPSNR score using ffmpeg.
pub fn run(
    reference: &Path,
    distorted: &Path,
    filter_complex: &str,
) -> anyhow::Result<impl Stream<Item = XpsnrOut>> {
    info!(
        "xpsnr {} vs reference {}",
        distorted.file_name().and_then(|n| n.to_str()).unwrap_or(""),
        reference.file_name().and_then(|n| n.to_str()).unwrap_or(""),
    );

    let mut cmd = Command::new("ffmpeg");
    cmd.arg2("-i", distorted)
        .arg2("-i", reference)
        .arg2("-filter_complex", filter_complex)
        .arg2("-f", "null")
        .arg("-")
        .stdin(Stdio::null());

    let cmd_str = cmd.to_cmd_str();
    debug!("cmd `{cmd_str}`");
    let mut xpsnr = ProcessChunkStream::try_from(cmd).context("ffmpeg xpsnr")?;

    Ok(async_stream::stream! {
        let mut chunks = Chunks::default();
        let mut parsed_done = false;
        while let Some(next) = xpsnr.next().await {
            match next {
                Item::Stderr(chunk) => {
                    if let Some(out) = XpsnrOut::try_from_chunk(&chunk, &mut chunks) {
                        if matches!(out, XpsnrOut::Done(_)) {
                            parsed_done = true;
                        }
                        yield out;
                    }
                }
                Item::Stdout(_) => {}
                Item::Done(code) => {
                    if let Err(err) = exit_ok_stderr("ffmpeg xpsnr", code, &cmd_str, &chunks) {
                        yield XpsnrOut::Err(err);
                    }
                }
            }
        }
        if !parsed_done {
            yield XpsnrOut::Err(cmd_err(
                "could not parse ffmpeg xpsnr score",
                &cmd_str,
                &chunks,
            ));
        }
    })
}

#[derive(Debug)]
pub enum XpsnrOut {
    Progress(FfmpegOut),
    Done(f32),
    Err(anyhow::Error),
}

impl XpsnrOut {
    fn try_from_chunk(chunk: &[u8], chunks: &mut Chunks) -> Option<Self> {
        chunks.push(chunk);

        if let Some(score) = chunks.rfind_line_map(score_from_line) {
            return Some(Self::Done(score));
        }
        if let Some(progress) = FfmpegOut::try_parse(chunks.last_line()) {
            return Some(Self::Progress(progress));
        }
        None
    }
}

// E.g. "[Parsed_xpsnr_0 @ 0x711494004cc0] XPSNR  y: 33.6547  u: 41.8741  v: 42.2571  (minimum: 33.6547)"
fn score_from_line(line: &str) -> Option<f32> {
    const MIN_PREFIX: &str = "minimum: ";

    if !line.contains("XPSNR") {
        return None;
    }

    let yidx = line.find(MIN_PREFIX)?;
    let tail = &line[yidx + MIN_PREFIX.len()..];
    let end_idx = tail
        .char_indices()
        .take_while(|(_, c)| *c == '.' || c.is_numeric())
        .last()?
        .0;
    tail[..=end_idx].parse().ok()
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn parse_rgb_line() {
        let score = score_from_line(
            "XPSNR average, 1 frames  r: 40.6130  g: 41.0275  b: 40.6961  (minimum: 40.6130)",
        );
        assert_eq!(score, Some(40.6130));
    }

    #[test]
    fn parse_xpsnr_score() {
        // Note: some lines omitted for brevity
        const FFMPEG_OUT: &str = r#"Input #0, matroska,webm, from 'tmp.mkv':
  Metadata:
    COMPATIBLE_BRANDS: isomiso2avc1mp41
    MAJOR_BRAND     : isom
    MINOR_VERSION   : 512
    ENCODER         : Lavf61.7.100
  Duration: 00:00:53.77, start: -0.007000, bitrate: 2698 kb/s
  Stream #0:0(eng): Video: av1 (libdav1d) (Main), yuv420p10le(tv, progressive), 3840x2160, 25 fps, 25 tbr, 1k tbn (default)
      Metadata:
        HANDLER_NAME    : ?Mainconcept Video Media Handler
        VENDOR_ID       : [0][0][0][0]
        ENCODER         : Lavc61.19.100 libsvtav1
        DURATION        : 00:00:53.760000000
  Stream #0:1(eng): Audio: opus, 48000 Hz, stereo, fltp (default)
      Metadata:
        title           : Opus 96Kbps
        HANDLER_NAME    : #Mainconcept MP4 Sound Media Handler
        VENDOR_ID       : [0][0][0][0]
        ENCODER         : Lavc61.19.100 libopus
        DURATION        : 00:00:53.768000000
Input #1, mov,mp4,m4a,3gp,3g2,mj2, from 'pixabay-lemon-82602.mp4':
  Metadata:
    major_brand     : isom
    minor_version   : 512
    compatible_brands: isomiso2avc1mp41
    encoder         : Lavf58.20.100
  Duration: 00:00:53.76, start: 0.000000, bitrate: 14109 kb/s
  Stream #1:0[0x1](eng): Video: h264 (High) (avc1 / 0x31637661), yuv420p(progressive), 3840x2160, 14101 kb/s, 25 fps, 25 tbr, 12800 tbn (default)
      Metadata:
        handler_name    : ?Mainconcept Video Media Handler
        vendor_id       : [0][0][0][0]
  Stream #1:1[0x2](eng): Audio: aac (LC) (mp4a / 0x6134706D), 48000 Hz, stereo, fltp, 2 kb/s (default)
      Metadata:
        handler_name    : #Mainconcept MP4 Sound Media Handler
        vendor_id       : [0][0][0][0]
Stream mapping:
  Stream #0:0 (libdav1d) -> xpsnr
  Stream #1:0 (h264) -> xpsnr
  xpsnr:default -> Stream #0:0 (wrapped_avframe)
  Stream #0:1 -> #0:1 (opus (native) -> pcm_s16le (native))
Press [q] to stop, [?] for help
[Parsed_xpsnr_0 @ 0x78341c004d00] not matching timebases found between first input: 1/1000 and second input 1/12800, results may be incorrect!
Output #0, null, to 'pipe:':
  Metadata:
    COMPATIBLE_BRANDS: isomiso2avc1mp41
    MAJOR_BRAND     : isom
    MINOR_VERSION   : 512
    encoder         : Lavf61.7.100
  Stream #0:0: Video: wrapped_avframe, yuv420p10le(tv, progressive), 3840x2160 [SAR 1:1 DAR 16:9], q=2-31, 200 kb/s, 25 fps, 25 tbn
      Metadata:
        encoder         : Lavc61.19.100 wrapped_avframe
  Stream #0:1(eng): Audio: pcm_s16le, 48000 Hz, stereo, s16, 1536 kb/s (default)
      Metadata:
        title           : Opus 96Kbps
        HANDLER_NAME    : #Mainconcept MP4 Sound Media Handler
        VENDOR_ID       : [0][0][0][0]
        DURATION        : 00:00:53.768000000
        encoder         : Lavc61.19.100 pcm_s16le
frame=    9 fps=0.0 q=-0.0 size=N/A time=00:00:00.32 bitrate=N/A speed=0.64x    
frame=   28 fps= 28 q=-0.0 size=N/A time=00:00:01.08 bitrate=N/A speed=1.08x    
frame=   46 fps= 31 q=-0.0 size=N/A time=00:00:01.80 bitrate=N/A speed= 1.2x    
frame=   65 fps= 32 q=-0.0 size=N/A time=00:00:02.56 bitrate=N/A speed=1.28x    
n:    1  XPSNR y: 54.5266  XPSNR u: 56.3886  XPSNR v: 58.7794
n:    2  XPSNR y: 40.6035  XPSNR u: 39.3487  XPSNR v: 42.3634
n:    3  XPSNR y: 40.9764  XPSNR u: 38.8791  XPSNR v: 41.8961
n:   64  XPSNR y: 41.0726  XPSNR u: 39.7731  XPSNR v: 42.5210
n:   65  XPSNR y: 41.3476  XPSNR u: 39.6055  XPSNR v: 42.4262
n:   66  XPSNR y: 41.1029  XPSNR u: 39.8779  XPSNR v: 42.6400
frame=   84 fps= 34 q=-0.0 size=N/A time=00:00:03.32 bitrate=N/A speed=1.33x    
frame=  102 fps= 34 q=-0.0 size=N/A time=00:00:04.04 bitrate=N/A speed=1.35x    
frame=  120 fps= 34 q=-0.0 size=N/A time=00:00:04.76 bitrate=N/A speed=1.36x    
n:   67  XPSNR y: 40.9642  XPSNR u: 39.5204  XPSNR v: 42.1316
n:   68  XPSNR y: 40.2677  XPSNR u: 38.9371  XPSNR v: 41.9560
n:   69  XPSNR y: 40.6431  XPSNR u: 38.8864  XPSNR v: 41.6902
n: 1319  XPSNR y: 41.4316  XPSNR u: 40.5146  XPSNR v: 42.1970
n: 1320  XPSNR y: 41.4623  XPSNR u: 40.5527  XPSNR v: 42.3358
n: 1321  XPSNR y: 42.5312  XPSNR u: 41.2487  XPSNR v: 42.8495
frame= 1328 fps= 37 q=-0.0 size=N/A time=00:00:53.08 bitrate=N/A speed=1.47x    
[Parsed_xpsnr_0 @ 0x78341c004d00] XPSNR  y: 40.7139  u: 39.1440  v: 41.7907  (minimum: 39.1440)
[out#0/null @ 0x64006e11b1c0] video:578KiB audio:10080KiB subtitle:0KiB other streams:0KiB global headers:0KiB muxing overhead: unknown
frame= 1344 fps= 37 q=-0.0 Lsize=N/A time=00:00:53.72 bitrate=N/A speed=1.48x    
n: 1342  XPSNR y: 40.6841  XPSNR u: 39.0209  XPSNR v: 40.9250
n: 1343  XPSNR y: 41.0269  XPSNR u: 39.2465  XPSNR v: 41.1238
n: 1344  XPSNR y: 39.8468  XPSNR u: 38.4587  XPSNR v: 40.5844

XPSNR average, 1344 frames  y: 40.7139
"#;

        const CHUNK_SIZE: usize = 64;

        let ffmpeg = FFMPEG_OUT.as_bytes();

        let mut chunks = Chunks::default();
        let mut start_idx = 0;
        let mut xpsnr_score = None;
        while start_idx < ffmpeg.len() {
            let chunk = &ffmpeg[start_idx..(start_idx + CHUNK_SIZE).min(FFMPEG_OUT.len())];
            // println!("* {}", String::from_utf8_lossy(chunk).trim());

            if let Some(xpsnr) = XpsnrOut::try_from_chunk(chunk, &mut chunks) {
                println!("{xpsnr:?}");
                if let XpsnrOut::Done(score) = xpsnr {
                    xpsnr_score = Some(score);
                }
            }

            start_idx += CHUNK_SIZE;
        }

        assert_eq!(xpsnr_score, Some(39.1440), "failed to parse xpsnr score");
    }
}
