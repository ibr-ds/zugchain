FROM ubuntu:focal
ENV DEBIAN_FRONTEND noninteractive
RUN apt-get update -y && apt-get install -y \
    g++\
    clang\
    libclang-dev\
    cmake\
    make\
    curl\
    openssh-client \
    git \
    && rm -rf /var/lib/apt/lists/*

ADD rust-toolchain .
RUN curl https://sh.rustup.rs -sSf | sh -s -- -y --profile minimal --default-toolchain none
ENV PATH ${PATH}:~/.cargo/bin/

RUN ~/.cargo/bin/rustup toolchain install "$(cat rust-toolchain)"