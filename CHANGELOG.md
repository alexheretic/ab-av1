# Unreleased (v0.2.0)
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
