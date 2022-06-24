# railchain

## Building and Running
First steps
- `git clone https://gitlab.ibr.cs.tu-bs.de/ds-railchain/railchain.git`
- `cd railchain`
- `git submodule update --init`

Blockchain compare and plot tools:
- `cargo run -p rc-cli -- --help`

## Deploy on VT605 (smartrail)

### Requirements:
- Rustup (Toolchain Manager) https://rustup.rs/
- Cross (Docker-based cross compiling for Rust/Cargo) https://github.com/rust-embedded/cross `cargo install cross`
- Gitlab Acces to https://gitlab.ibr.cs.tu-bs.de/ds-railchain/railchain Container Registry
- docker, ansible, rsync

### What goes where?
- `railchain` binary goes on the smartrails (debian armv7) and runs raily consensus and blockchain
- `export2` binary goes on macbook (debian x86_64) and runs on a timer to export blocks to the database

### Compiling

Compiling has to be done before executing ansible.

**Ansible does not compile before uploading/running.**

#### `railchain`

We use `cross` to to cross compile `railchain` for arm in a docker container with the appropriate toolchain.
`./Cross.toml` configures the used image for the target.
`osfstream` has to be activated to receive osf blocks over UDP.

`cross build -Zbuild-std --target armv7-smartrail-linux-gnueabihf.json --release --bin railchain --features=osfstream`

#### `export2`

We use `cross` to compile in a docker container with `ubuntu:focal` so it works on the macbook.
This isn't cross compiling but it's convenient because you have to compile against a compatible glibc version.
If yours it too new the binary will not work on the target system. So we use `cross` to get a stable image.

`cross build --release --target x86_64-unknown-linux-gnu --bin export2 --features database`

If you're on Ubuntu 20.04, Debian 10 or older you can run cargo directly instead:

`cargo build --release --bin export2 --features database`


### Deployment

We use ansible over ssh to deploy the system. Ansible files can be found in `ansible/`

**Ansible does not compile before uploading/running.**

#### ssh
Add the contents `ansible/ssh-config` to your ssh config (`~/.ssh/config`).
Ask Kai for the `omservice` Key and Password to the Jump Host if you don't have it.

#### ansible

Files referenced in the playbooks can be found in `ansible/files` or `ansible/templates` for `j2` templates.

- **Upload Raily**:
    - `ansible-playbook -i ansible/smartrail.ini ansible/smartrail.yaml`
- **Start Raily**:  #
    - `ansible-playbook -i ansible/smartrail.ini --tags start ansible/smartrail_control.yaml`
- **Stop Raily**:
    - `ansible-playbook -i ansible/smartrail.ini --tags stop ansible/smartrail_control.yaml`
- **Manually Download Blocks** (to current directory):
    - `rsync 'rc-sr-01:raily/runchain/*' .`
- **Upload Export**:
    - `ansible-playbook -i ansible/smartrail.ini ansible/smartrail_export.yaml`


### Docker
You can build the images yourself:
- **smartrail build**:
    - `docker build -f docker/smartrail.Dockerfile -t gitlab.ibr.cs.tu-bs.de:4567/ds-railchain/railchain/smartrail-build`
- **x86_64 build**:
    - `docker build -f docker/build.Dockerfile -t gitlab.ibr.cs.tu-bs.de:4567/ds-railchain/railchain/build`

The tag names used have to match `Cross.toml`.

## Other Deployments
### Building for the mvb simulator (on a x86_64 system)

- make sure a x86 linker and 32 bit glibc is available,
- `rustup target add i686-unknown-linux-gnu`
- `cargo build --target i686-unknown-linux-gnu`

### Build for mcom

#### Docker

- install the `cross` tool either from your distro repos or `cargo install cross`.
- make sure you can access the container registry at gitlab.ibr.cs.tu-bs.de:4567/ds-railchain/railchain
- or build the container: `docker build -t gitlab.ibr.cs.tu-bs.de:4567/ds-railchain/railchain/mcom-build:latest -f docker/mcom.Dockerfile .` (see Cross.toml for the tag name)
- `cross build -Zbuild-std --target armv7-smartrail-linux-gnueabihf.json --release --bin railchain`

## Deploy on mcoms

Various ansible scripts deploy configuration on hosts such as the mcoms. Check out `ansible/sim-fm2.yaml` for the most recent deployment.