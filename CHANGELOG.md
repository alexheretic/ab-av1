# Unreleased (v0.3.0)
* Add `--downmix-to-stereo` option, if enabled & the input streams use > 3 channels (dts 5.1 etc), 
  downmix input audio streams to stereo.
* After encoding print per-stream sizes in addition to the file size & percent.
* When defaulting the output file don't use input extension if it is _avi, y4m, ivf_, use mp4 instead.
* Add `--vmaf-width` option which sets the video resolution width to use in VMAF analysis.
* When using the default VMAF model, improve VMAF accuracy for sub-1k resolutions by defaulting
  `--vmaf-width=1920` whenever video resolution width is less than 1728. This will result in lower 
  VMAF scores than were reported for such videos in previous versions.
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
