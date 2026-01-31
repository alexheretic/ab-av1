# Messages output when using `--stdout-format json`

## sample-encode
Single json emitted after sample encoding has finished.

* `xpsnr` is present when using --xpsnr (in that case `vmaf` will be absent).

### Example
```json
{
  "predicted_encode_percent": 41.01556024884758,
  "predicted_encode_seconds": 33,
  "predicted_encode_size": 38889644,
  "vmaf": 95.13105773925781
}
```
