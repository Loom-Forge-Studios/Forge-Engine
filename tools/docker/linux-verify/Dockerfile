# Forge Linux verify environment (ADR 0044, WP-30): the ubuntu-x86_64 leg of `just verify`
# observed locally in Docker. Built and run by `just verify-linux`
# (tools/docker/linux-verify/verify-linux.ps1 on Windows, verify-linux.sh elsewhere).
#
# Bandwidth rules (the owner is on a shared, metered link):
#   * The base image is pinned by digest and is never re-pulled: it is the rust:1.98.1-bookworm
#     image already in the local store (the repo toolchain is 1.98.1, rust-toolchain.toml).
#     No `# syntax=` line: that would pull a Dockerfile frontend image.
#   * apt and the two rustup components are the only downloads, once, at image build.
#   * The cargo tools are compiled from the HOST's crate cache: the scripts pass the host's
#     `registry/cache` and `registry/index` as the named build contexts `hostcache` and
#     `hostindex`, and the install runs with CARGO_NET_OFFLINE=true, so a missing crate fails
#     the build instead of downloading it. The bind mounts are discarded after the step.
#     A Windows host's cache lacks the tools' Linux-only crates the first time; ADR 0044 gives
#     the one-off `cargo fetch --target x86_64-unknown-linux-gnu` that adds exactly those.
FROM rust:1.98.1-bookworm@sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e

ENV DEBIAN_FRONTEND=noninteractive

# What the build and the tests need beyond the base image (build-essential, git, pkg-config
# and the X11 headers are already in it):
#   mesa-vulkan-drivers + libvulkan1  lavapipe, the software Vulkan the GPU tests and goldens
#                                     render on (NOT a hardware Linux GPU)
#   vulkan-tools                      vulkaninfo, to record which Vulkan device was observed
#   xvfb + xauth                      an X server, so the X11 clipboard test (arboard) and
#                                     winit have a display instead of reporting AWAITING
#   libxkbcommon*, libx*, libwayland  what winit dlopens at run time
#   rsync                             incremental copy of the worktree snapshot into the
#                                     volume (unchanged files keep their mtimes)
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      mesa-vulkan-drivers libvulkan1 vulkan-tools \
      xvfb xauth \
      libxkbcommon0 libxkbcommon-x11-0 libx11-xcb1 libxcursor1 libxrandr2 libxi6 \
      libwayland-client0 \
      rsync \
 && rm -rf /var/lib/apt/lists/*

# rust-toolchain.toml asks for rustfmt and clippy; the base image's minimal profile lacks them.
RUN rustup component add rustfmt clippy

# The same tool versions the Windows dev machine runs, built offline from the host's crates.
RUN --mount=type=bind,from=hostcache,target=/usr/local/cargo/registry/cache,rw \
    --mount=type=bind,from=hostindex,target=/usr/local/cargo/registry/index,rw \
    CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=6 \
    cargo install --locked just@1.58.0 cargo-nextest@0.9.145 cargo-deny@0.20.2 \
 && CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=6 \
    cargo install --locked --features cli cargo-about@0.9.2 \
 && rm -rf /usr/local/cargo/registry/src /usr/local/cargo/registry/CACHEDIR.TAG

# System fonts a Linux desktop has and the text tests rely on (a separate layer so adding it
# did not re-download the layers above): a colour emoji font (the glyph atlas's colour plane,
# ui_single_glyph_atlas) and a CJK font (the 600-ideograph atlas-epoch stress page). The
# bundled Roboto/Droid set (D-7) has neither; the system font DB is their fallback.
RUN apt-get update \
 && apt-get install -y --no-install-recommends fonts-noto-color-emoji fonts-wqy-microhei \
 && rm -rf /var/lib/apt/lists/*

# AT-SPI, the Linux accessibility bus (WP-U12, DoD M2-22), in its own layer (the layers above
# stay cached): at-spi2-core (the accessibility bus launcher and the registry) and dbus (a
# session bus to start them on). tools/docker/linux-verify/atspi-probe.sh runs them in a private
# session and queries ui_gallery's tree over the bus, as a screen reader does.
RUN apt-get update \
 && apt-get install -y --no-install-recommends at-spi2-core dbus \
 && rm -rf /var/lib/apt/lists/*

# A runtime dir, so winit/xkbcommon do not complain that XDG_RUNTIME_DIR is unset.
RUN mkdir -p -m 700 /run/user/0
ENV XDG_RUNTIME_DIR=/run/user/0

# The toolchain's bytes as built, checked by in-container.sh before every run. On 2026-09-23
# the WSL2 VM's page cache held a corrupted copy of librustc_driver.so (every rustc
# codegen SIGSEGV'd, even hello world, in a pristine base-image container; dropping the page
# cache restored the on-disk bytes and the build). A run on corrupted tools must stop with
# that diagnosis, not report compiler crashes as engine failures.
RUN sha256sum /usr/local/rustup/toolchains/*/lib/*.so /usr/local/rustup/toolchains/*/bin/* \
      /usr/local/cargo/bin/* > /etc/forge-toolchain.sha256

# Software Vulkan only: lavapipe's ICD is the one Vulkan device in the container.
ENV VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.x86_64.json \
    CARGO_TERM_COLOR=always \
    CARGO_INCREMENTAL=0

WORKDIR /vol/src

# libudev's headers and pkg-config file (WP-65, ADR 0064): gilrs reads gamepads on Linux through
# evdev with udev hot-plug, and its libudev-sys links libudev at build time. Its own layer, last,
# so every layer above stays cached (a rebuild downloads this one package).
RUN apt-get update \
 && apt-get install -y --no-install-recommends libudev-dev \
 && rm -rf /var/lib/apt/lists/*
