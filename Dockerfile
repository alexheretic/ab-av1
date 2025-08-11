FROM rust:1-slim-trixie as builder

WORKDIR /build

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN apt-get update && apt-get install -y wget xz-utils

# fetch ffmpeg bin
RUN dpkgArch="$(dpkg --print-architecture)" \
    && case "${dpkgArch##*-}" in \
        amd64) wget https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-n7.1-latest-linux64-gpl-7.1.tar.xz -O /tmp/ffmpeg.tar.xz && \
                tar -xvf /tmp/ffmpeg.tar.xz  && cd ffmpeg-n7.1-latest-linux64-gpl-7.1/bin && mv ffmpeg ffprobe /build/ ;; \
        arm64) wget https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-n7.1-latest-linuxarm64-gpl-7.1.tar.xz -O /tmp/ffmpeg.tar.xz && \
                tar -xvf /tmp/ffmpeg.tar.xz  && cd ffmpeg-n7.1-latest-linuxarm64-gpl-7.1/bin && mv ffmpeg ffprobe /build/ ;; \
        *) echo "Unsupported architecture: ${dpkgArch}"; exit 1 ;; \
    esac

RUN rustup default stable
RUN cargo build --release

FROM debian:trixie-slim as runtime

COPY --from=builder /build/ffmpeg /build/ffprobe /usr/local/bin/
COPY --from=builder /build/target/release/ab-av1 /app/

WORKDIR /videos

ENTRYPOINT ["/app/ab-av1"]
