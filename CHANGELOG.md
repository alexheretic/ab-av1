# Unreleased (v0.4.0)
* Add `--encoder`/`-e` encoder override. 
  Any [encoder ffmpeg supports](https://ffmpeg.org/ffmpeg-all.html#toc-Video-Encoders)
  and that may be controlled using `-crf` may be used.
* Add `--enc $FFMPEG_ARG` for providing arbitrary output options to the ffmpeg encoder invocation.
  These only work when overriding the encoder with `-e`.
  <br/>_E.g. Set x265 params: `-e libx265 --enc x265-params=lossless=1`._
* Add `--enc-input $FFMPEG_ARG` for providing ffmpeg input file options, similar to `--enc`.
* `--preset` now supports also word presets like `slow`, `veryfast` for ffmpeg encoders like libx264.
* `--preset` is **no longer required**. Default svt-av1 `--preset` is now **8**.
* Support setting keyint for `-e` encoders in a similar way as is done for av1.
* Add default vp9 & libaom-av1 `-b:v 0` setting so constant quality crf based encoding works consistently.
* For `-e libaom-av1` map `--preset` number to `-cpu-used` (0-8).
* Shell escape file name in "Encoding ..." output.

# v0.3.4
* Shell escape file names when hinting commands.

# v0.3.3
* Show more info when auto-encode fails to find a suitable crf.

# v0.3.2
* Improve sample generation speed & frame duration accuracy.

# v0.3.1
* Fix some cases where ffmpeg progress & VMAF score output parsing failed.
* Fix some edge cases where crf-search would succeed exceeding the specified `--max-encoded-percent`.

# v0.3.0
* Select vmaf model `model=version=vmaf_4k_v0.6.1` for videos larger than 2560x1440 if no other model is specified.
  This will raise VMAF scores for 4k videos that previously were getting harsher treatment from the 1k model.
* Add `--vmaf-scale` option which sets the video resolution scale to use in VMAF analysis.
  `auto` (default) auto scales based on model & resolution, `none` no scaling or custom `WxH`
  format, e.g. `1920x1080`.
  - `auto` upscale 1728x972 & smaller to 1080p, preserving aspect, when using the default 1k VMAF model.
    This will reduce VMAF scores that previously were getting more generous treatment from the 1k model.
  - `auto` upscale 3456x1944 & smaller to 4k, preserving aspect, when using the 4k VMAF model.
* Add `--downmix-to-stereo` option, if enabled & the input streams use > 3 channels (dts 5.1 etc), 
  downmix input audio streams to stereo.
* After encoding print per-stream sizes in addition to the file size & percent.
* Add predicted video stream percent reduction to _auto-encode_ search progress bar after a successful search.
* Support non-video/audio/subtitle streams from input to output, e.g. attachments.
* When defaulting the output file don't use input extension if it is _avi, y4m, ivf_, use mp4 instead.
* Fix clearing _crf-search_ progress bar output on error.
* Strip debug symbols in release builds by default which reduces binary size _(requires rustc 1.59)_.

# v0.2.0
* Add svt-av1 option `--keyint FRAME-OR-DURATION` argument supporting frame integer or duration string. 
  _E.g. `--keyint=300` or `--keyint=10s`_.
  Default keyint to `10s` when input duration is over 3m.
* Add svt-av1 option `--scd true|false` argument enabling scene change detection.
  Default scd on when using default keyint & input duration is over 3m.
* Add `--svt ARG` for additional args, _e.g. `--svt mbr=2000 --svt film-grain=30`_.
* Add `--vfilter ARG` argument to apply a ffmpeg video filter (crop, scale etc) to the input before av1 encoding.
  <br/>_E.g. `--vfilter "scale=1280:-1,fps=24"`_.
* Add `--pix-format ARG` argument supporting `yuv420p10le` (default) & `yuv420p`.
* Add vmaf configuration `--vmaf ARG`, _e.g. `--vmaf n_threads=8 --vmaf n_subsample=4`_.
* Rename _vmaf_ command argument `--reference` (was `--original`).
* Add _vmaf_ command `--reference-vfilter` argument, similar to `--vfilter`.
* Default vmaf n_threads to the number of logical CPUs.
* Add `--temp-dir` argument to specify storage of sample data. 
  May also be set with env var `AB_AV1_TEMP_DIR`.
* Add `--sample-every DURATION` argument, default "12m".
* Remove 3 sample default, this is now calculated using `--sample-every` 12m default.
* Create samples concurrently while encoding to reduce io lags waiting to encode.
* _crf-search_ re-use samples for crf analysis.
* Linux: _vmaf_ use fifo to convert both reference & distorted to yuv which fixes vmaf accuracy in some cases.
* Support multiple audio & subtitle streams.
* Use 128k bitrate as a default for libopus audio.
* Remove `--aq`.
* Fail fast if ffmpeg cut samples are empty (< 1K).
* Handle input durations lower than the sample duration by using the whole input as a single sample.

# v0.1.1
* Add command to generate bash,fish & zsh completions `ab-av1 print-completions [SHELL]`.

# v0.1.0
* Initial release.
