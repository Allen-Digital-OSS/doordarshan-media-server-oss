FROM rust:1.87.0-slim-bookworm AS builder

# Install basic tools
RUN apt-get update && apt-get install -y --no-install-recommends \
    curl wget apt-transport-https ca-certificates gnupg \
    build-essential python3 python3-pip valgrind pkg-config libssl-dev \
    net-tools git iputils-ping procps sudo \
    && rm -rf /var/lib/apt/lists/*

# Add Debian Sid source for GStreamer 1.26.x
RUN echo "deb http://deb.debian.org/debian sid main" > /etc/apt/sources.list.d/sid.list

# Configure apt pinning
RUN printf '%s\n' \
  "Package: gstreamer1.0*" \
  "Pin: release a=sid" \
  "Pin-Priority: 500" \
  "" \
  "Package: libgstreamer*" \
  "Pin: release a=sid" \
  "Pin-Priority: 500" \
  "" \
  "Package: gst-plugins-*" \
  "Pin: release a=sid" \
  "Pin-Priority: 500" \
  "" \
  "Package: *" \
  "Pin: release a=bookworm" \
  "Pin-Priority: 990" \
  > /etc/apt/preferences.d/gst-pin

# Install GStreamer 1.26.x and dev headers
RUN apt-get update && apt-get install -y --no-install-recommends \
    gstreamer1.0-tools \
    gstreamer1.0-plugins-base \
    gstreamer1.0-plugins-good \
    gstreamer1.0-plugins-bad \
    gstreamer1.0-plugins-ugly \
    gstreamer1.0-libav \
    gstreamer1.0-x \
    gstreamer1.0-gl \
    gstreamer1.0-alsa \
    gstreamer1.0-pulseaudio \
    libgstreamer1.0-dev \
    libgstreamer-plugins-base1.0-dev \
    libglib2.0-dev \
    && rm -rf /var/lib/apt/lists/*

# Print GStreamer version for confirmation
RUN gst-launch-1.0 --version

# Install cargo-chef
RUN cargo install cargo-chef
RUN rustup component add rustfmt
ENV RUST_BACKTRACE=full

# Set up build environment
WORKDIR /doordarshan-media-server
ENV RUSTFLAGS="--cfg tokio_unstable"

# Copy needed directories
COPY ./src /doordarshan-media-server/src
#COPY ./config /doordarshan-media-server/config
COPY ./Cargo.lock /doordarshan-media-server/Cargo.lock
COPY ./Cargo.toml /doordarshan-media-server/Cargo.toml

# Build the binary
RUN cargo build --release

RUN mkdir -p /app
RUN mkdir -p /app/config
RUN mkdir -p /var/log
RUN touch /var/log/doordarshan-media-server.log
RUN chmod 777 /var/log/doordarshan-media-server.log

RUN cp target/release/doordarshan-media-server /app/
WORKDIR /app
#COPY ./config/* /app/config/
COPY ./run.sh ./run.sh

RUN chmod +x run.sh

EXPOSE 3001
EXPOSE 40000-45000/udp
EXPOSE 40000-40100/tcp
EXPOSE 6669
# EXPOSE 80
RUN echo "MY_ENV_VAR is set to: $ENV"

ENTRYPOINT ["sh", "run.sh"]
