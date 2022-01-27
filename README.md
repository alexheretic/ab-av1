# ab-av1
AV1 re-encoding using _ffmpeg_, _svt-av1_ & _vmaf_.

## sample-encode
Encode short video samples of an input using provided **crf** & **preset**. 
This is much quicker than full encode/vmaf run. 

Outputs:
* Mean sample VMAF score
* Predicted full encode size
* Predicted full encode time

```
ab-av1 sample-encode [OPTIONS] -i <INPUT> --crf <CRF> --preset <PRESET>
```

## encode
Simple invocation of ffmpeg & SvtAv1EncApp to reencode a video.

```
ab-av1 encode [OPTIONS] -i <INPUT> --crf <CRF> --preset <PRESET>
```

## vmaf
Simple full calculation of VMAF score distorted file vs original file.

```
ab-av1 vmaf --original <ORIGINAL> --distorted <DISTORTED>
```
