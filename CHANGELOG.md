# v0.10.1
* Support setting `--enc-input hwaccel=none --enc-input hwaccel_output_format=none` to omit defaults
  for *_vaapi, *_vulkan vcodecs introduced in v0.9.4.

# v0.10.0
* `--pix-format` no longer generally defaults to "yuv420p", instead if not specified no -pix_fmt 
  will be passed to ffmpeg allowing use of upstream defaults.
  However, libsvtav1, libaom-av1 & librav1e will continue to default to "yuv420p10le".
* Allow specifying ffmpeg decoder using `--enc-input c:v=CODEC`.

# v0.9.4
* Encoder *_vaapi: Default args `--enc-input hwaccel=vaapi --enc-input hwaccel_output_format=vaapi`.
* Encoder *_vulkan: Map `--crf` to ffmpeg `-qp`.
* Encoder *_vulkan: Default args `--enc-input hwaccel=vulkan --enc-input hwaccel_output_format=vulkan`.
* Encoder libvvenc: Map `--crf` to ffmpeg `-qp`.

# v0.9.3
* Support setting per-stream audio codec, e.g. `--enc c:a:1=libopus`.
* Support `--pix-format yuv422p10le`.
* Write video stream metadata "AB_AV1_FFMPEG_ARGS" to encoded output, include a subset of relevant 
  ffmpeg args used. E.g. `AB_AV1_FFMPEG_ARGS: -c:v libsvtav1 -crf 25 -preset 8`. 
  (Not supported by mp4 files).

# v0.9.2
* Log crf results, instead of printing, if stderr is not a terminal.
* Wait for all child processes (ffmpeg etc) to finish before temp file cleanup and exit.

# v0.9.1
* Fix xpsnr inf score parsing.
* Fix xpsnr reference vfilter usage.
* Add `--xpsnr-fps`: Frame rate override used to analyse both reference & distorted videos. Default 60.

# v0.9.0
* Add XPSNR support as a VMAF alternative.
  - Add sample-encode `--xpsnr` arg which toggles use of XPSNR instead of VMAF.
  - Add crf-search, auto-encode `--min-xpsnr` arg _(alternative to `--min-vmaf`)_.
  - Add `xpsnr` command for measuring XPSNR score.
* Support negative `--preset` args.
* Add `--vmaf-fps`: Frame rate override used to analyse both reference & distorted videos. Default 25.
* Omit data streams when outputting to matroska (.mkv or .webm).
* Omit audio, subtitle & data streams in VMAF calls to work around possible ffmpeg memory leaks.
* mpeg2video: map `--crf` to ffmpeg `-q` and set default crf range to 2-30.

# v0.8.0
* crf-search: Tweak 2nd iteration logic that slices the crf range at the 25% or 75% crf point.
  - Widen to 20%/80% to account for searches of the "middle" two subranges being more optimal.
  - Disable when using custom min/max crf ranges under half the default.
* Add sample-encode info to crf-search & auto-encode. Show sample progress and encoding/vmaf fps.
* Improve sample-encode progress format consistency.
* Add crf-search `-v` flag to print per-sample results.
* Add auto-encode `-v` flag to print per-crf results, `-vv` to also print per-sample results.

# v0.7.19
* Fix stdin handling sometimes breaking bash shells.

# v0.7.18
* Use default .265, .264 image output extensions for libx265, libx264.
  Fixes crf-search for images with these codecs.
* Improve `--vfilter` docs, clarify VMAF usage.

# v0.7.17
* Improve failing ffmpeg stderr printing: 
  - Don't allow many '\r'-ending updates to cause all other stored info to be truncated. 
  - Increase max heap storage of output ~4k->32k to allow more complete output in some cases.
* Fix caching unaffected by `--reference-vfilter` usage.
* Improve `--vfilter` docs. Describe VMAF usage & mention `--reference-vfilter`.
* Improve `--vmaf-scale` docs.
* VMAF: Remove `-r 24` ffmpeg input.
* VMAF: Add new default options "shortest=true", "ts_sync_mode=nearest" and use vfilter "settb=AVTB".

# v0.7.16
* Fix VMAF score parse failure of certain successful ffmpeg outputs.

# v0.7.15
* Show full ffmpeg command after errors.
* For *_vaapi encoders map `--crf` to ffmpeg `-q` (instead of `-qp`).
* Set av1_vaapi default `--max-crf` to 255.
* Fix sample-encode printing output to non-terminals.
* Omit "Encode with: ..." stderr hint for non-terminals.
* Support logging enabled when stderr is not a terminal or by setting env var `RUST_LOG`. E.g:
  - `RUST_LOG=ab_av1=info` "info" level logs various progress results like sample encode info
  - `RUST_LOG=ab_av1=debug` "debug" level logs include ffmpeg calls
* Don't panic on non-zero status exit.
* When unable to parse a vmaf score fail faster and include ffmpeg output.
* Add `--reference-vfilter` arg to _sample-encode_, _crf-search_, _auto-encode_ to allow
  overriding `--vfilter` for VMAF.
* Add `--sample-duration` arg to configure the duration of each sample. Default 20s.

# v0.7.14
* Fix bash completions of some filenames.

# v0.7.13
* Use a single ffmpeg process to calculate VMAF replacing multi process piping.
* Exclude subtitle tracks from samples.
* Add `--keep` option for _crf-search_ & _auto-encode_.

# v0.7.12
* Improve eta stability.

# v0.7.11
* Fix sample-encode caching to consider vmaf args.

# v0.7.10
* Fix validation preventing use of svt args starting with "-i", "-b".

# v0.7.9
* Fix validation preventing use of ffmpeg --enc args starting with "-i", e.g. "-init_hw_device".

# v0.7.8
* Fix ETA calculation overflow panic scenario.

# v0.7.7
* Add `--video-only` option for _encode_ & _auto-encode_.

# v0.7.6
* Fix nested temp directories not being cleaned properly.
* Temp directories will now start with "." and be created in the working dir instead of the input parent
  (unless setting --temp-dir).

# v0.7.5
* Add `-e librav1e` support. Map `--crf` to ffmpeg `-qp` (default max 255), `--preset` to `-speed` (0-10).
* Disallow `--enc svtav1-params=` usage. libsvtav1 params should instead be set with `--svt`.

# v0.7.4
* Add `--encoder` support for qsv family of ffmpeg encoders: av1_qsv, hevc_qsv, vp9_qsv, h264_qsv and mpeg2_qsv.
* Enable lookahead mode by default for encoders: av1_qsv, hevc_qsv, h264_qsv.

# v0.7.3
* Include all other non-main video streams by copying instead of encoding them with the same
  settings as the main video stream.
* Always copy audio unless `--acodec` or `--downmix-to-stereo` are specified. Previously would
  re-encode to opus when changing container.

# v0.7.2
* Print failing ffmpeg stderr output.
* Preserve all input file streams (e.g. audio, subs, attachments) into output.
* Support concurrent running processes out of the box by segregating temp-dirs & fixing cache access.
* Improve vmaf accuracy in some cases by forcing 24fps & synchronizing the presentation timestamp.
* Automatically workaround ffmpeg _"Can't write packet with unknown timestamp"_ sample generation failures
  (typically encountered with old avi files) by using \`-fflags +genpts\`.

# v0.7.1
* Fix _crf-search_ incorrectly picking a rate that exceeds the `--max-encoded-percent`.
* Improve _auto-encode_ crf float display rounding.

# v0.7.0
* Use ffmpeg for svt-av1 encodes instead of invoking to SvtAv1EncApp directly. This unifies the handling of
  other encoders & allows svt-av1 encoding to benefit from more built-in ffmpeg behaviours like aspect preservation.<br/>
  **An ffmpeg build with libsvtav1 enabled is now required**. SvtAv1EncApp is no longer required.
* Improve image detection.
* Add `--encoder` support for nvenc family of ffmpeg encoders: av1_nvenc, hevc_nvenc, and h264_nvenc.

# v0.6.1
* Add _sample-encode_, _crf-search_, _auto-encode_ arg `--min-samples`.
* Revert libvpx-vp9 `--crf-increment` default to **1**.

# v0.6.0
* Support decimal crf values in _sample-encode_, _encode_ subcommands (note svt-av1 only supports integer crf).
* Add _crf-search_, _auto-encode_ arg `--crf-increment`. Previously this would always be 1.
  Defaults to **1**. -e libx264, libx265 & libvpx-vp9 default to **0.1**.
* Add _crf-search_, _auto-encode_ arg `--thorough` which more exhaustively searches to find
  a crf value close to the specified min-vmaf.
* Cache _sample-encode_ results in $CACHE_DIR/ab-av1 directory. This allows repeated same crf sample encoding
  to be avoided when running _sample-encode_, _crf-search_ & _auto-encode_. E.g. repeating a _crf-search_ with
  a different min-vmaf.<br/>
  Caching is enabled by default. Can be disabled with `--cache false` or setting env var `AB_AV1_CACHE=false`.
* Use mkv containers for all lossless samples. Previously mp4 samples were used for mp4 inputs, however in all test cases
  mkv 20s samples were better quality. This change improves accuracy for all mp4 input files.
* Default `--max-crf` to **46** for libx264 & libx265 encoders.
* Encode webm outputs with the "cues" seek index at the front to optimise stream usage (as done with mkv).

# v0.5.2
* Fix ffprobe duration conversion error scenarios panicking.
* Tweak encoded size prediction logic to consider both input file size & encoded sample duration.

# v0.5.1
* Change encoded size prediction logic to estimate video stream size (or image size) only.
  This should be much more consistent than the previous method. 
  Change _crf-search_, _sample-encode_ result text to clarify this.
* Improve video size prediction logic to account for samples that do not turn out as 20s.
* Fix full-pass sample encode progress bar.
* Use label "Full pass" instead of "Sample 1/1" when doing a full pass _sample-encode_.
* Add VMAF auto model, n_threads & scaling documentation.

# v0.5.0
* Default to .mkv output format for all inputs (except .mp4 which will continue to output .mp4 by default).
  This also applies to ffmpeg encoder sample output format. The previous behaviour used the input extension
  which may not have supported av1 (e.g. .m2ts).
* For _auto-encode_ use the output extension also for ffmpeg encoder sample outputs if applicable.
* When creating lossless samples for encode analysis use .mkv (or .mp4) extension for better ffmpeg compatibility.
* Encode mkv outputs with the "cues" seek index at the front to optimise stream usage.
* Optimise pixel format choice for VMAF comparisons. Can significantly improve VMAF fps.
  _E.g. if both videos are yuv420p use that instead of yuv444p10le_.
* When sampling use full input video when sample time would be >= 85% of the total (down from 100%).
* Eliminate repeated redundant ffprobe calls.
* Windows: Support VMAF pixel format conversion for both distorted and reference.
  Gives more consistently accurate results and brings Windows in line with Linux functionality.
* Windows: ab-av1.exe binaries will now be automatically built and attached to releases.

# v0.4.4
* Add _crf-search_, _auto-encode_, _encode_ & _vmaf_ command support for encoding images into avif.
  This works in the same way as videos, example:
  ```
  ab-av1 auto-encode -i pic.jpg
  ```
  The default encoder svt-av1 has some dimension limitations which may cause this to fail. `-e libaom-av1` also works and supports more dimensions.
* Convert to yuv444p10le pixel format when calculating VMAF for accuracy and compatibility.
* Update to clap v4 which changes help/about output & reduces binary size.
* Print _crf-search_ attempts even when stderr is not a tty.

# v0.4.3
* Fix terminal breaking sometimes after exiting early.

# v0.4.2
* Update _indicatif_ dependency to `0.17`.

# v0.4.1
* For `-e libvpx-vp9` map `--preset` number to ffmpeg `-cpu-used` (0-5).
* When overriding with a ffmpeg encoder avoid setting `b:a`, `movflags` or `ac` if explicitly set via `--enc`.
* Add error output when using `--enc-input` with the default svt-av1 encoder.
* Add errors for `--enc`/`--enc-input` args that are already provided by existing args or inferred.

# v0.4.0
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
* For `-e libaom-av1` map `--preset` number to ffmpeg `-cpu-used` (0-8).
* For *_vaapi encoders map `--crf` to ffmpeg `-qp` as crf is not supported.
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
