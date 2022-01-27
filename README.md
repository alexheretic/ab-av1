# ab-av1
AV1 re-encoding using _ffmpeg_, _svt-av1_ & _vmaf_.

## sample-vmaf
Fast VMAF score for provided AV1 re-encoding settings. Uses short video samples to avoid expensive
full duration encoding & vmaf calculation. Also predicts encoding size & duration

```
USAGE:
    ab-av1 sample-vmaf [OPTIONS] -i <INPUT> --crf <CRF> --preset <PRESET>

OPTIONS:
        --crf <CRF>            Encoder constant rate factor. Lower means better quality
    -h, --help                 Print help information
    -i, --input <INPUT>        Input video file
        --preset <PRESET>      Encoder preset. Higher presets means faster encodes, but with a
                               quality tradeoff
        --samples <SAMPLES>    Number of 20s samples [default: 3]
```

## encode
Simple invocation of ffmpeg & SvtAv1EncApp to reencode a video.

```
USAGE:
    ab-av1 encode [OPTIONS] -i <INPUT> --crf <CRF> --preset <PRESET>

OPTIONS:
        --crf <CRF>          Encoder constant rate factor. Lower means better quality
    -h, --help               Print help information
    -i, --input <INPUT>      Input video file
    -o, --output <OUTPUT>    Output file, by default the same as input with `.av1.mp4` extension
        --preset <PRESET>    Encoder preset. Higher presets means faster encodes, but with a quality
                             tradeoff
```

## vmaf
Simple full calculation of VMAF score distorted file vs original file.

```
USAGE:
    ab-av1 vmaf --original <ORIGINAL> --distorted <DISTORTED>

OPTIONS:
        --distorted <DISTORTED>    Re-encoded/distorted video file
    -h, --help                     Print help information
        --original <ORIGINAL>      Original video file
```
