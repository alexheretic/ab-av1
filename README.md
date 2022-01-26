# abav1
AV1 re-encoding using _ffmpeg_ & _svt-av1_ & _vmaf_.

## sample-vmaf
Fast calculation of VMAF score for AV1 re-encoding settings using short samples.

```
USAGE:
    abav1 sample-vmaf [OPTIONS] -i <INPUT> --crf <CRF> --preset <PRESET>

OPTIONS:
        --crf <CRF>            Encoder constant rate factor. Lower means better quality
    -h, --help                 Print help information
    -i <INPUT>                 Input video file
        --keep                 Keep temporary files after exiting
        --preset <PRESET>      Encoder preset. Higher presets means faster encodes, but with a
                               quality tradeoff
    -q, --quiet                Don't print verbose progress info
        --samples <SAMPLES>    Number of 20s samples [default: 3]
```
