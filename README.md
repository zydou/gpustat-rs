# gpustat-rs

An independent Rust rewrite of [gpustat](https://github.com/wookayin/gpustat),
for NVIDIA GPUs. Same command line, idiomatic Rust.

中文文档见 [README.zh-Hans.md](README.zh-Hans.md).

The crate is called `gpustat-rs`; the installed binary is called `gpustat`, so
it drops into an existing installation in place.

```sh
gpustat                 # one line per GPU
gpustat --json          # full records as JSON
gpustat -a -f           # every property, including full command lines and CPU usage
gpustat -i 1            # watch mode, one frame per second
gpustat --help
```

`--show-all` covers everything except the full command line; `-f` adds it.
`-i` and `--json` are mutually exclusive.

```text
dnkt                        Mon Oct 26 20:54:32 2026  575.57.08
[0] NVIDIA GeForce RTX 4090 | 41°C,   0 % | 21575 / 24564 MB | root(21552M) gdm(4M)
[1] NVIDIA GeForce RTX 4090 | 40°C,   0 % | 21574 / 24564 MB | root(21552M) gdm(4M)
[2] NVIDIA GeForce RTX 4090 | 41°C,   0 % | 21574 / 24564 MB | root(21552M) gdm(4M)
[3] NVIDIA GeForce RTX 4090 | 34°C,   0 % | 21574 / 24564 MB | root(21552M) gdm(4M)
```

## Install

Prebuilt binaries are on the [Releases](https://github.com/zydou/gpustat-rs/releases)
page: tagged releases, plus `nightly`, refreshed on every push to main.

The artifact is a single binary dynamically linked against `libc.so.6` and
`libgcc_s.so.1`, with no Python. It needs glibc >= 2.34 (Debian 12 / Ubuntu
22.04 and newer).

```sh
tar -xJf gpustat-x86_64-unknown-linux-gnu.tar.xz
sudo install -Dm755 gpustat /usr/local/bin/gpustat
```

The archive also carries `LICENSE`. The install step overwrites
`/usr/local/bin/gpustat` without asking; if the Python tool is installed from
pip, `pip uninstall gpustat` first, since both print `gpustat X.Y.Z` and are
otherwise indistinguishable.

Linux x86_64 only. No macOS, no aarch64.

## Build

```sh
cargo build --release --locked
```

The toolchain comes from `rust-toolchain.toml` (stable, with `rustfmt` and
`clippy`). For a portable artifact use cross instead:

```sh
cross build --release --locked --target x86_64-unknown-linux-gnu
```

## Notes

- Only the NVIDIA backend is implemented. `--backend` still accepts `auto`,
  `nvidia`, `amd` and `ascend`; the last two report `the amd backend is not
  built into this binary` and exit 1. A named vendor should not be answered
  with a different vendor's devices.
- `--json` keeps the upstream dotted key names (`temperature.gpu`,
  `enforced.power.limit`, ...), so existing `jq` scripts keep working.
- This is a rewrite, not a byte-identical port. stdout, stderr and exit codes
  are aligned with upstream where it matters, but are not guaranteed to match,
  and upstream bugs are deliberately not reproduced. `--help` and parse errors
  use `clap`'s layout, JSON is indented with two spaces, and `-P limit` shows
  the limit only. A per-implementation record lives in `CLAUDE.md`, which is
  for contributors.

## License

[MIT](LICENSE).
