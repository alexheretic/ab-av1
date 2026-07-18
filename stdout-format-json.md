# Messages output when using `--stdout-format json`

Commands supporting `--stdout-format json` write newline-delimited JSON ([NDJSON](https://github.com/ndjson/ndjson-spec)) to stdout: one object per line, each with a `type` key identifying the message kind. Progress bars, logs & hints go to stderr only, so stdout is parseable line by line.

Supported by: `sample-encode`, `crf-search`.

Notes:
* Key order is not significant (currently alphabetical, so `type` is typically last).
* Later versions may add keys & message kinds, consumers should ignore unknown ones.
* Failures print an `Error: ...` line to stderr and exit non-zero, in json mode too. The only failure also reported as json is a failed crf-search, see [`crf-search-error`](#crf-search-error).

## `sample-encode-done`
Emitted after all samples of a sample encode run are encoded & scored. `sample-encode` emits one at the end of the run, `crf-search` one per crf attempt.

Field | Description | Type/Units
---|---|---
`type` | `"sample-encode-done"` | string
`crf` | Encoder crf used | float
`from_cache` | All sample results were read from the cache | bool
`predicted_encode_percent` | Predicted output encode size percentage vs input | float
`predicted_encode_seconds` | Predicted output encode time in seconds | float
`predicted_encode_size` | Predicted output encode size in bytes | uint
`vmaf` | Mean sample VMAF score (present when requested (default)) | float
`xpsnr` | Mean sample XPSNR score (present when requested) | float

### Example
```json
{"crf":29.25,"from_cache":false,"predicted_encode_percent":14.819840772420237,"predicted_encode_seconds":12.0,"predicted_encode_size":73692619,"type":"sample-encode-done","vmaf":95.37376403808594}
```

Note: In ≤ v0.11.4 `sample-encode` emitted this object without the `type`, `crf` & `from_cache` keys.

## `crf-search-attempt`
Emitted after each crf attempt that did not successfully end the search, like the human `- crf 34 VMAF 95.13 (41%)` line. Follows the attempt's `sample-encode-done`, which carries the full predictions.

Field | Description | Type/Units
---|---|---
`type` | `"crf-search-attempt"` | string
`crf` | Attempted crf | float
`from_cache` | All sample results were read from the cache | bool
`predicted_encode_percent` | Predicted output encode size percentage vs input | float
`vmaf` | Mean sample VMAF score (present when requested (default)) | float
`xpsnr` | Mean sample XPSNR score (present when requested) | float

### Example
```json
{"crf":29.25,"from_cache":false,"predicted_encode_percent":14.819840772420237,"type":"crf-search-attempt","vmaf":95.37376403808594}
```

## `crf-search-done`
Emitted when the search ends successfully, like the final human result line. A successful final attempt gets no `crf-search-attempt`.

Note: The best crf may have been decided by an earlier attempt, in which case this message repeats that attempt's values and is not adjacent to its `sample-encode-done`.

Field | Description | Type/Units
---|---|---
`type` | `"crf-search-done"` | string
`crf` | Best crf found | float
`from_cache` | All sample results were read from the cache | bool
`predicted_encode_percent` | Predicted output encode size percentage vs input | float
`predicted_encode_seconds` | Predicted output encode time in seconds | float
`predicted_encode_size` | Predicted output encode size in bytes | uint
`vmaf` | Mean sample VMAF score (present when requested (default)) | float
`xpsnr` | Mean sample XPSNR score (present when requested) | float

### Example
```json
{"crf":29.75,"from_cache":false,"predicted_encode_percent":13.785497397240093,"predicted_encode_seconds":13.0,"predicted_encode_size":68549279,"type":"crf-search-done","vmaf":95.16242980957031}
```

## `crf-search-error`
Emitted when the search fails to find a crf satisfying the min score & max encoded percent. Follows the failing attempt's `sample-encode-done` & `crf-search-attempt`. As in human mode, `Error: Failed to find a suitable crf` goes to stderr and the exit code is non-zero.

Field | Description | Type/Units
---|---|---
`type` | `"crf-search-error"` | string
`message` | Failure description | string

### Example
```json
{"message":"Failed to find a suitable crf","type":"crf-search-error"}
```

## `sample-encode` output
A single `sample-encode-done`.

## `crf-search` output
`sample-encode-done` + `crf-search-attempt` per unsuccessful attempt, ending with `crf-search-done` (exit 0) or `crf-search-error` (non-zero exit).

Guarantees:
* Exactly one `sample-encode-done` per crf attempted.
* The final line is a `crf-search-done` or `crf-search-error`. Other errors (e.g. invalid input) end the stream with no final json message: stderr `Error:` line & non-zero exit only.

### Example: successful search
```json
{"crf":37.5,"from_cache":false,"predicted_encode_percent":3.9154531401777755,"predicted_encode_seconds":9.0,"predicted_encode_size":19469845,"type":"sample-encode-done","vmaf":90.38392639160156}
{"crf":37.5,"from_cache":false,"predicted_encode_percent":3.9154531401777755,"type":"crf-search-attempt","vmaf":90.38392639160156}
{"crf":29.75,"from_cache":false,"predicted_encode_percent":13.785497397240093,"predicted_encode_seconds":13.0,"predicted_encode_size":68549279,"type":"sample-encode-done","vmaf":95.16242980957031}
{"crf":29.75,"from_cache":false,"predicted_encode_percent":13.785497397240093,"predicted_encode_seconds":13.0,"predicted_encode_size":68549279,"type":"crf-search-done","vmaf":95.16242980957031}
```

### Example: failed search
```json
{"crf":18.0,"from_cache":false,"predicted_encode_percent":58.12225504159517,"predicted_encode_seconds":18.0,"predicted_encode_size":289016681,"type":"sample-encode-done","vmaf":98.99139404296875}
{"crf":18.0,"from_cache":false,"predicted_encode_percent":58.12225504159517,"type":"crf-search-attempt","vmaf":98.99139404296875}
{"message":"Failed to find a suitable crf","type":"crf-search-error"}
```
