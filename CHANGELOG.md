# Unreleased (v0.2.0)
* Add optional `--vmaf-options` argument to _vmaf, sample-encode, crf-search, auto-encode_ commands.
* Fail fast if ffmpeg cut samples are empty (< 1K).
* Handle input durations lower than the sample duration by using the whole input as a single sample.

# v0.1.1
* Add command to generate bash,fish & zsh completions `ab-av1 print-completions [SHELL]`.

# v0.1.0
* Initial release.
