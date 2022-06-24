# xenial for gcc-4.9, xenial = 16.04
FROM ubuntu:focal

#super hax for gcc 4.9
RUN echo "deb http://dk.archive.ubuntu.com/ubuntu/ xenial main \n\
deb http://dk.archive.ubuntu.com/ubuntu/ xenial universe\n\
deb http://security.ubuntu.com/ubuntu xenial-security main\n \
deb http://security.ubuntu.com/ubuntu xenial-security universe\n" >> /etc/apt/sources.list

RUN apt-get update -y && apt-get install -fy \
    g++-4.9-arm-linux-gnueabihf \
    gcc-4.9-arm-linux-gnueabihf=4.9.3-13ubuntu2cross1 \
    libstdc++-4.9-dev-armhf-cross=4.9.3-13ubuntu2cross1 \
    libstdc++6-armhf-cross=5.4.0-6ubuntu1~16.04.9cross1 \
    libgcc-4.9-dev-armhf-cross=4.9.3-13ubuntu2cross1 \
    libatomic1-armhf-cross=5.4.0-6ubuntu1~16.04.9cross1\
    libgomp1-armhf-cross=5.4.0-6ubuntu1~16.04.9cross1\
    libc6-armhf-cross=2.23-0ubuntu3cross1 \
    libc6-dev-armhf-cross=2.23-0ubuntu3cross1 \
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

# set c/c++ compilers,
ENV CARGO_TARGET_ARMV7_MCOM_LINUX_GNUEABIHF_LINKER=arm-linux-gnueabihf-gcc-4.9 \
    CC_armv7_mcom_linux_gnueabihf=arm-linux-gnueabihf-gcc-4.9 \
    CXX_armv7_mcom_linux_gnueabihf=arm-linux-gnueabihf-g++-4.9 \
    CARGO_TARGET_ARMV7_MCOM_LINUX_GNUEABIHF_RUSTFLAGS="-C target-cpu=cortex-a9 -C target-feature=+neon"

# install toolchain used in project + arm target
ADD rust-toolchain .
RUN rustup toolchain install "$(cat rust-toolchain)" && \
    rustup target add --toolchain "$(cat rust-toolchain)" armv7-unknown-linux-gnueabihf
