# focal for ubuntu on raspi
FROM ubuntu:focal

RUN apt-get update -y && apt-get install -y \
    g++-10-aarch64-linux-gnu \
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
    rustup target add --toolchain "$(cat rust-toolchain)" aarch64-unknown-linux-gnu

# set c/c++ compilers,
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc-10 \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc-10 \
    CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++-10