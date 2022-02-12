# Unreleased (v0.2.0)
* Add svt-av1 arguments:
  - `--keyint FRAME-OR-DURATION` argument supporting frame integer or duration string, _e.g. `--keyint=300` or `--keyint=10s`_.
  - `--scd` argument enabling scene change detection.
  - `--svt ARG` for additional args, _e.g. `--svt mbr=2000 --svt film-grain=30`_.
* Rename _vmaf_ command argument `--reference` (was `--original`).
* Use 128k bitrate as a default for libopus audio.
* Remove `--aq`.
* Add optional `--vmaf-options` argument to _vmaf, sample-encode, crf-search, auto-encode_ commands.
* Fail fast if ffmpeg cut samples are empty (< 1K).
* Handle input durations lower than the sample duration by using the whole input as a single sample.

# v0.1.1
* Add command to generate bash,fish & zsh completions `ab-av1 print-completions [SHELL]`.

# v0.1.0
* Initial release.
