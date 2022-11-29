# ab-av1
AV1 video encoding tool with fast VMAF sampling & automatic encoder crf calculation. 
Uses _ffmpeg_, _svt-av1_ & _vmaf_.

![](https://user-images.githubusercontent.com/2331607/151695971-d36f55a7-a157-4d5d-ae06-4cc9e2c0d46f.png "Find the best crf encoding setting for VMAF 95 quality")

Also supports other ffmpeg compatible encoders like libx265 & libx264.

### Command: auto-encode
Automatically determine the best crf to deliver the min-vmaf and use it to encode a video or image.

Two phases:
* [crf-search](#command-crf-search) to determine the best --crf value
* ffmpeg & SvtAv1EncApp to encode using the settings

```
ab-av1 auto-encode [OPTIONS] -i <INPUT> --preset <PRESET>
```

### Command: crf-search
Interpolated binary search using [sample-encode](#command-sample-encode) to find the best 
crf value delivering **min-vmaf** & **max-encoded-percent**.

Outputs:
* Best crf value
* Mean sample VMAF score
* Predicted full encode size
* Predicted full encode time

```
ab-av1 crf-search [OPTIONS] -i <INPUT> --preset <PRESET>
```

### Command: sample-encode
Encode short video samples of an input using provided **crf** & **preset**. 
This is much quicker than full encode/vmaf run. 

Outputs:
* Mean sample VMAF score
* Predicted full encode size
* Predicted full encode time

```
ab-av1 sample-encode [OPTIONS] -i <INPUT> --crf <CRF> --preset <PRESET>
```

### Command: encode
Simple invocation of ffmpeg & SvtAv1EncApp to encode a video or image.

```
ab-av1 encode [OPTIONS] -i <INPUT> --crf <CRF> --preset <PRESET>
```

### Command: vmaf
Simple full calculation of VMAF score distorted file vs reference file.

Works with videos and images.

```
ab-av1 vmaf --reference <REFERENCE> --distorted <DISTORTED>
```

## Install
### Arch Linux
Available in the [AUR](https://aur.archlinux.org/packages/ab-av1).

### Windows
Pre-built **ab-av1.exe** included in the [latest release](https://github.com/alexheretic/ab-av1/releases/latest).

### Using cargo
Latest release
```sh
cargo install ab-av1
```

Latest code direct from git
```sh
cargo install --git https://github.com/alexheretic/ab-av1
```

### Requirements
* svt-av1
* ffmpeg
* vmaf
* opus

`ffmpeg`, `SvtAv1EncApp` commands should be in `$PATH`.

## Minimum supported rust compiler
Maintained with [latest stable rust](https://gist.github.com/alexheretic/d1e98d8433b602e57f5d0a9637927e0c).
