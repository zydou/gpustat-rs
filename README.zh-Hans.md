# gpustat-rs

[gpustat](https://github.com/wookayin/gpustat) 的独立 Rust 重写，面向 NVIDIA
GPU。同样的命令行，Rust 的地道实现。

英文文档见 [README.md](README.md)。

crate 叫 `gpustat-rs`，安装出的可执行文件叫 `gpustat`，因此可以原地替换已有的
安装。

```sh
gpustat                 # 每块 GPU 一行
gpustat --json          # 完整条目以 JSON 输出
gpustat -a -f           # 全部属性，含完整命令行与 CPU 占用
gpustat -i 1            # watch 模式，间隔 1 秒
gpustat --help
```

`--show-all` 覆盖除完整命令行以外的全部属性，`-f` 才会加上它。`-i` 与 `--json`
互斥。

```text
dnkt                        Mon Oct 26 20:54:32 2026  575.57.08
[0] NVIDIA GeForce RTX 4090 | 41°C,   0 % | 21575 / 24564 MB | root(21552M) gdm(4M)
[1] NVIDIA GeForce RTX 4090 | 40°C,   0 % | 21574 / 24564 MB | root(21552M) gdm(4M)
[2] NVIDIA GeForce RTX 4090 | 41°C,   0 % | 21574 / 24564 MB | root(21552M) gdm(4M)
[3] NVIDIA GeForce RTX 4090 | 34°C,   0 % | 21574 / 24564 MB | root(21552M) gdm(4M)
```

## 安装

发布产物附在 [Releases](https://github.com/zydou/gpustat-rs/releases) 页面：
打 tag 发布的正式版本，以及每次推送 main 都会刷新的 `nightly`。

产物是一个动态链接 `libc.so.6` 与 `libgcc_s.so.1` 的单个二进制，无需 Python
环境。需要 glibc >= 2.34（Debian 12 / Ubuntu 22.04 及更新版本）。

```sh
tar -xJf gpustat-x86_64-unknown-linux-gnu.tar.xz
sudo install -Dm755 gpustat /usr/local/bin/gpustat
```

归档里还带着 `LICENSE`。上面的安装命令会不询问地覆盖
`/usr/local/bin/gpustat`；如果之前用 pip 装过 Python 版，请先
`pip uninstall gpustat`，因为两者都输出 `gpustat X.Y.Z`，除此之外无从区分。

只支持 Linux x86_64，不支持 macOS 与 aarch64。

## 构建

```sh
cargo build --release --locked
```

工具链由 `rust-toolchain.toml` 选定（stable，含 `rustfmt` 与 `clippy`）。

需要注意可移植性时请改用 cross 构建：

```sh
cross build --release --locked --target x86_64-unknown-linux-gnu
```

## 说明

- 只有 NVIDIA 后端已实现。`--backend` 仍接受 `auto` / `nvidia` / `amd` /
  `ascend`，后两者会报 `the amd backend is not built into this binary` 并以
  1 退出。点明了厂商，就不该用别的厂商的设备来回答。
- `--json` 的键名沿用上游的点号命名（`temperature.gpu`、
  `enforced.power.limit` 等），便于已有的 `jq` 脚本继续工作。
- 这是重写，不是逐字节移植。stdout / stderr / 退出码在要紧的地方与上游对齐，
  但不保证一致，上游自己的 bug 也刻意不复现：`--help` 与解析错误用 clap 的
  排版，JSON 用 2 空格缩进，`-P limit` 只显示 limit。逐条记录在 `CLAUDE.md`，
  那是面向贡献者的文档。

## 许可证

[MIT](LICENSE)。
