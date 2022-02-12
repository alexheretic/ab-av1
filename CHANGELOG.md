# Unreleased (v0.2.0)
* Add svt-av1 configuration:
  - `--keyint FRAME-OR-DURATION` argument supporting frame integer or duration string, 
    _e.g. `--keyint=300` or `--keyint=10s`_.
    Default keyint to `10s` when input duration is over 3m.
  - `--scd true|false` argument enabling scene change detection.
    Default scd on when input duration is over 3m.
  - `--svt ARG` for additional args, _e.g. `--svt mbr=2000 --svt film-grain=30`_.
* Add vmaf configuration `--vmaf ARG`, _e.g. `--vmaf n_threads=8 --vmaf n_subsample=4`_.
* Set vmaf n_threads to the number of logical CPUs by default.
* Rename _vmaf_ command argument `--reference` (was `--original`).
* Use 128k bitrate as a default for libopus audio.
* Remove `--aq`.
* Fail fast if ffmpeg cut samples are empty (< 1K).
* Handle input durations lower than the sample duration by using the whole input as a single sample.

# v0.1.1
* Add command to generate bash,fish & zsh completions `ab-av1 print-completions [SHELL]`.

# v0.1.0
* Initial release.
