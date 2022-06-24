FROM ubuntu:focal

RUN apt-get update -y && apt-get install -fy \
    g++-arm-linux-gnueabihf \
    curl \
    cmake \
    make \
    && rm -rf /var/lib/apt/lists/*

#bindgen
#RUN apt-get install -y libc6-dev-i386 libclang-5.0-dev clang-5.0

# rust install, no toolchain
RUN curl https://sh.rustup.rs -sSf | \
    sh -s -- --default-toolchain none -y

ENV PATH=/root/.cargo/bin:$PATH

# install toolchain used in project + arm target
ADD rust-toolchain .
RUN rustup toolchain install "$(cat rust-toolchain)" && \
    rustup target add --toolchain "$(cat rust-toolchain)" armv7-unknown-linux-gnueabihf

# set c/c++ compilers,
ENV CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_LINKER=arm-linux-gnueabihf-gcc \
    CARGO_TARGET_armv7-smartrail_linux_gnueabihf.json_LINKER=arm-linux-gnueabihf-gcc \
    CC_armv7_unknown_linux_gnueabihf=arm-linux-gnueabihf-gcc \
    CXX_armv7_unknown_linux_gnueabihf=arm-linux-gnueabihf-g++ \
    CC_armv7_smartrail_linux_gnueabihf=arm-linux-gnueabihf-gcc \
    CXX_armv7_smartrail_linux_gnueabihf=arm-linux-gnueabihf-g++ \
    CARGO_TARGET_ARMV7_UNKNOWN_LINUX_GNUEABIHF_RUSTFLAGS="-C target-cpu=cortex-a9 -C target-feature=+neon"
