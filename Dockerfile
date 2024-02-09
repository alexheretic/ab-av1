FROM rust:latest as builder

WORKDIR /build

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo fetch
RUN cargo build --release

FROM debian:bookworm-slim as dependencies

RUN apt-get update && apt-get install -y \
    wget \
    xz-utils \
    && rm -rf /var/lib/apt/lists/*

RUN dpkgArch="$(dpkg --print-architecture)" \
    && case "${dpkgArch##*-}" in \
        amd64) wget https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-n6.1-latest-linux64-gpl-6.1.tar.xz -O /tmp/ffmpeg.tar.xz && \
                tar -xvf /tmp/ffmpeg.tar.xz  && cd ffmpeg-n6.1-latest-linux64-gpl-6.1/bin && mv ffmpeg ffprobe /usr/local/bin ;; \
        arm64) wget https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-n6.1-latest-linuxarm64-gpl-6.1.tar.xz -O /tmp/ffmpeg.tar.xz && \
                tar -xvf /tmp/ffmpeg.tar.xz  && cd ffmpeg-n6.1-latest-linuxarm64-gpl-6.1/bin && mv ffmpeg ffprobe /usr/local/bin ;; \
        *) echo "Unsupported architecture: ${dpkgArch}"; exit 1 ;; \
    esac

FROM dependencies as runtime

COPY --from=builder /build/target/release/ab-av1 /app/ab-av1

WORKDIR /videos

ENTRYPOINT ["/app/ab-av1"]
