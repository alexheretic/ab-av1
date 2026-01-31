# Messages output when using `--stdout-format json`

## sample-encode
Single json emitted after sample encoding has finished.

Field | Description | Type/Units
---|---|---
`predicted_encode_percent` | Predicted output encode size percentage vs input | float
`predicted_encode_seconds` | Predicted output encode time in seconds | float
`predicted_encode_size` | Predicted output encode size in bytes | uint
`vmaf` | VMAF score (absent when using --xpsnr) | float
`xpsnr` | XPSNR score (present only when using --xpsnr) | float

### Example
```json
{
  "predicted_encode_percent": 41.01556024884758,
  "predicted_encode_seconds": 33,
  "predicted_encode_size": 38889644,
  "vmaf": 95.13105773925781
}
```
