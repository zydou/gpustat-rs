# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## 项目性质

这是 Python 版 [gpustat](https://github.com/wookayi/gpustat) 的 **Rust 重写**，不是移植。
目标是功能正确、代码地道：同样的 argv 下能给出同样的信息，但**不要求** stdout /
stderr / 退出码逐字节一致。

**不要复刻 Python 的行为怪癖，也不要复刻上游的 bug。** 参考实现里的行为如果是
"因为 Python 那样"才成立的，这里就按正确的做法写。改代码前不需要担心破坏与
参考实现的一致性——一致性不是约束，可读性和正确性才是。

## 命令

```sh
cargo build --release --locked            # 本地构建
cargo run -- --json                       # 直接运行

cargo test --workspace                    # 全部测试（内联在各自模块里）
cargo test --lib plain_text_layout        # 按名字片段过滤单个测试

cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings

cross build --release --locked --target x86_64-unknown-linux-gnu   # 可移植性构建
```

`cargo fmt`、`clippy -D warnings`、`cargo test` 三条就是 CI 的门禁，全部必须干净。
CI 跑不了 NVML（GitHub runner 没有 NVIDIA 驱动），改过查询或渲染逻辑后请在本机
对着真实 GPU 跑一遍：默认表格、`-a -f`、`--json | jq`、`--id`、`--debug`、`--gpuname-width`。

## 架构

数据流：`main.rs` → `cli::run`（clap 解析 → `--show-all` 展开 → 分派）→
`model::GpuStatCollection::new_query` 经 `nvml::NvmlQuerier` 填模型 →
`format::print_formatted` / `print_json` 写 `std::io::stdout()`。
`-i/--interval` 把最后一步换成 `watch::run` 的循环。

| 模块 | 职责 |
| --- | --- |
| `cli.rs` | clap derive 的 `Args` + 分派 + 错误报告 |
| `nvml.rs` | `Querier` trait 与 `NvmlQuerier`（`nvml-wrapper`） |
| `model.rs` | `GpuInfo` / `GpuStat` / `GpuStatCollection` 与查询编排 |
| `process.rs` | 用 `/proc` 读用户名、cmdline、cpu%、RSS |
| `format.rs` | `render` / `print_formatted` / `json` / 颜色解析 |
| `util.rs` | `bytes2human`、`shorten_left`、`prettify_commandline` 等纯函数 |
| `palette.rs` | SGR 序列常量与 `Palette` |
| `watch.rs` | `-i` 的重绘循环 |

依赖只有 `libc`、`chrono`、`clap`、`nvml-wrapper`、`serde`、`serde_json`。
手写实现仅限于 gpustat 特有的输出格式；参数解析、JSON 序列化、颜色判断这些
通用能力一律用现成库。

## 必须保持的约定

这些不是对参考实现的行为复刻，而是本项目自己的正确性要求：

- **`GpuInfo` 的字段顺序即 JSON 键序**，键名是 gpustat 的公开 API
  （`temperature.gpu`、`enforced.power.limit` 等），脚本会按这些名字取值。
  改字段请同步检查 `format::tests::json_keeps_the_gpustat_key_names`。
- **`nvml.rs` 的错误粒度**：只有 device handle / name / uuid 三个查询失败才会
  让整块设备变成一行错误（错误信息直接放在名字的位置）；其余一律走 `safe()`
  降级为 `None` 并记进 `Log`，只有 `--debug` 才汇总打印。
- **多 GPU 上的同一进程只采样一次**：`cpu_percent` 是两次采样的差值，对同一个
  pid 连续取两次会得到约 0，所以 `resample_cpu` 要先去重再写入所有出现的设备。
  `resample_pids` 有专门测试。
- **`new_query` 会睡 100 ms**：psutil 风格的两点采样需要真实的 cpu% 增量。
- **ANSI 序列只从 `palette.rs` 取**，不要在其他地方写字面量。`Palette` 关闭时
  每个方法返回 `""`，调用方因此不必分支。
- **`format::render` 只构造字符串，不落盘也不写 fd**；`print_*` 负责写。测试
  直接断言 `render` 的返回值。
- **颜色优先级**：`--force-color` > `NO_COLOR` / `--no-color` > `FORCE_COLOR` /
  tty。显式命令行开关要压过环境变量，见 `choose()` 及其测试。
- **报错文字的颜色按目标 fd 决定**：`format::palette(opts, stderr.is_terminal())`。
  `gpustat > /dev/tty 2>/tmp/log` 这种用法下 stderr 不该带颜色。

## 版本号与包名

`Cargo.toml` 的 `version` 是唯一来源；`-v` / `--version` 用
`env!("CARGO_PKG_VERSION")` 读取后打印（`cli::run` 自己处理这个开关，
因此 clap 的 version flag 是关掉的）。不要在源码里再写一份版本号。

版本号是独立序列，从 1.0.0 起，**不跟随**上游的编号。上游现在在 1.1.x，下一个
很可能就是 1.2.x，而两个工具的 `--version` 都打印 `gpustat X.Y.Z`；跟了的话，
在一台同时装了两种实现的机器上就没法判断是谁在跑。1.0.0 声明的是本实现的
命令行契约稳定，与上游的进度无关。

crate 叫 `gpustat-rs`，可执行文件叫 `gpustat`。前者是因为 `gpustat` 在
crates.io 上已被另一个 GPL-2.0 项目占用，包名永久不可转让；后者是为了原地
替换上游的安装。改包名时 `[[bin]]` 的 `name` 必须保留 `gpustat`。

## 已知与参考实现不同的地方

这些是**有意为之**的，不要当成 bug 修掉：

- **`--help` 与解析错误的排版、措辞、顺序是 clap 的**，不是 argparse 的。
- **`-P limit` 只显示 limit**。参考实现会连 draw 一起打出来
  （`if 'limit' in show_power`），与它自己的帮助文本矛盾。
- **`-P/-e` 的组合是显式的**：`draw` / `limit` / `draw,limit`，而不是靠
  子串匹配。
- **`--gpuname-width` 没有位数上限**。旧版为了模仿 Python 的
  `Too many decimal digits in format string` 而人为拒绝八位以上的数字。
- **`--id` 指定的索引全部不存在时以 1 退出**，报
  `no GPU available: index N: <NVML 的原因>`，不打印 `?` 占位行。参考实现会把
  每块坏设备都画成一行仍然以 0 退出；索引是用户点名的，不会在本进程存活期间
  变成真设备。部分失败（`--id 0,99`）仍是 0，坏的那块照常以错误行显示。
  一次查询与 watch 模式都适用，watch 模式下否则会无限重画全空表格；watch 只在
  stderr 打印细节行，不带 `Error querying accelerator devices.` 的前缀。
  判据在 `model::GpuStatCollection::none_available`，规则在 `cli::all_named_failed`。
- **`--debug` 不再伪造 Python traceback**；`--interval` 不再有 `PyTime_t`
  溢出文案。要详情就打印 Rust 侧的真实错误。
- **表头缺失的驱动版本显示 `unknown`**，不是 Python 的 `None`。
- **JSON 用 2 空格缩进**（`serde_json::to_string_pretty` 的默认值）。
- **stdout 用 `std::io::stdout()` 的默认缓冲**，不复刻 CPython 的分块写策略。
  `main.rs` 把 `SIGPIPE` 恢复成 `SIG_DFL`，所以管道读端提前关闭仍然干净退出。
- **watch 模式用"覆盖 + 清行"，不进备用屏幕缓冲**：`Ctrl+C` 时最后一帧留在
  屏上，不需要恢复。若以后想进备用缓冲，必须同时装一个在退出时还原屏幕的
  `SIGINT` 处理器。
- **AMD / Ascend 后端未实现**：`--backend amd|ascend` 报
  `the amd backend is not built into this binary` 并以 1 退出。
