# Linux ARM64 build/run · 2026-10-04

本轮已完成默认 features 的两端 release 构建、真实进程启动/停止/重启和必要 Linux 协议/runtime 测试。**这是当前 Linux ARM64 容器独立实测，不是 Pi／声学／执行器验收。**完整命令、退出码、输入、产物及测试身份在 [linux-summary.json](linux-summary.json)。

## 已核对输入与边界

- [x] `codex/architecture-refactor`，开始时 HEAD `8c17422`、前次源码 `8f5bb83`；现有 R04 Linux 命令仅为两包 `cargo check --no-default-features --all-targets --locked --offline -j2`。其通过不代表默认 features、最终链接、运行或 Pi 实测。
- [x] 按两包 Cargo.toml 核对默认 features；Server 包含现有 `channel-embedded`、`turn-easy` 及模型/provider 后端，Embedded 为 `wakeword-sherpa`、`stt-sherpa`、`direct-response`，间接启用 Sherpa 1.13.1。不把 `--all-targets` 写成所有 features。
- [x] 保留现有镜像 `sha256:6cd7a7a06976f8651cdb57fcde457e9dc7e2abe20587900dd750e1f373a25c70`，Linux ARM64 native Rust 1.95；ALSA 1.2.8、DBus 1.14.10、protobuf 3.21.12、OpenSSL 3.0.20 已存在，不安装系统包。
- [x] 新容器 `alia-review-linux-20261004-v1` 以 `--network container:alia-review-20261004-db` 使用本轮独立测试网络，未修改用户网络、5433、旧任务 PostgreSQL 或旧验证容器；完成后已停止。
- [x] 新 cache 使用前次 cache 的 APFS 写时复制，旧快照/缓存不修改。Root 冻结后才复制633输入；manifest SHA-256 为 `a1c7dc3d6787cf2b86144b60c17b5aaaaad65025d1695a1326194c826c8ba0d0`，host/container 前后校验以及最终工作区校验均无差异。源码提交为 `010f7aa5ae2c39a5c90c7f22979391a6b1bbbf23`，Root [逐项核对](committed-source-verification.json) 全部匹配。
- [x] 宿主相关缓存及镜像中没有 Linux native archive；按已授权的既有依赖恢复官方 `sherpa-onnx-v1.13.1-linux-aarch64-static-lib.tar.bz2`，19,167,687 字节，SHA-256 `86d0d3818770d4f6a087e52691d3ae171393972030f133df064a271fe9fe1749`，与旧记录相同。无依赖升级、无模型下载、无 `DOCS_RS` 链接绕过。

命令、退出码、版本、原始日志与 native 下载核对保存在 `run/validation-20261004/linux-arm64-v1/evidence/`；前置只读记录在 `run/validation-20261004/linux-preflight/readiness.json`。前置记录中“尚未运行”的状态只代表当时，不覆盖下文已完成结果。

## 构建、输入与产物

- [x] 解析默认 feature 的 metadata/tree，并保存实际 release library fingerprint；两种证据分别保留，不由 target 种类反推 features。
- [x] Cargo.lock SHA-256 `845d709734b382d24baaec28bd9d69a524e92c4b41366184de02f0ef7a322dfc`，原 proto、build.rs、配置模板、迁移和嵌入资源随633输入冻结。临时运行配置、验证脚本、Sherpa archive 与其 build script 另记摘要。
- [x] `RUST_TEST_THREADS`、`RUST_MIN_STACK`、`RUSTFLAGS`、`DOCS_RS` 均实际未设置。Linux 为 native aarch64，kernel `7.0.14-orbstack-00380-ga7e0a2dc9535`、glibc 2.36，不能称为 Ubuntu Pi 真机。

| 命令 | 退出码 | 墙钟耗时 | 结果 |
| --- | --- | --- | --- |
| `cargo build -p alia-server --release --locked --offline -j2` | 0 | 184.857秒 | 默认 features，最终代码生成/链接完成 |
| `cargo build -p alia-embedded --release --locked --offline -j2` | 0 | 85.590秒 | 默认 Sherpa static / direct-response，最终代码生成/链接完成 |

不是冷缓存性能测量。Server 实际 features 为 `channel-embedded, default, embed-openai, insights, llm-anthropic, llm-gemini, llm-openai, plugin-home-assistant, stt-whisper, tools-http, tts-sovits, turn-easy`；Embedded 为 `default, direct-response, sherpa-onnx, stt-sherpa, wakeword-sherpa`。没有启用 `vision-opencv`。编入既有 provider 不意味着配置启用或已对外调用；本轮未启用付费服务、HA 或模型推理。

两产物均为 ARM aarch64 ELF64 PIE、动态解释器 `/lib/ld-linux-aarch64.so.1`；`file`、`readelf`、`ldd` 全部 exit 0，未发现缺失动态库。Sherpa 的 KWS、offline STT、VAD 三个 native 创建符号在最终 Embedded ELF 中存在，且实际 build cache 内的 native archive 再核对仍为原 SHA。

| 产物 | 字节数 | SHA-256 |
| --- | ---: | --- |
| `cache/target/release/alia-server` | 46,020,472 | `f6dba03d98b473413bfdf29d9c002718956741716a9919bc8265d361d0a5effc` |
| `cache/target/release/alia-embedded` | 50,408,280 | `e8179ef176b38a91af8cad7dd4a67fc8209284f9676682a7e23ae9205673fb10` |

Server 直接动态依赖包含 OpenSSL 3、libcrypto、libgcc、libm、libc；Embedded 包含 libstdc++、ALSA、DBus 及其系统库。完整解析路径见 `alia-{server,embedded}-ldd.log`，实际列表和 fingerprint 在汇总中。两产物最终 SHA 与启动前一致。

## 真实 release 运行

- [x] `python3 /evidence/release-smoke.py` exit 0，1.135秒；执行两个完整启动/停止轮次，并在第二轮读取第一轮真实持久状态。
- [x] 临时配置选择已有 noop 模型/音频槽，关闭 discovery/硬件；实际生产 binary、PostgreSQL、SQLite、HTTP 与 gRPC 均保留。没有 Bind、已有设备绑定变更或输出动作。
- [x] `/health` 200、临时账户真实注册→重启后登录；两轮 Companion 均为 shadow，政策 revision/内容未改变。
- [x] 真实 Server REST `/api/v1/device/status` 调用 Embedded 公开 `GetBindingStatus`：online=true，遥测 null；另经直接公开 gRPC 确认仍未绑定。匿名 `ObserveControl` 返回 Unauthenticated（16）。可达和绑定/输出授权分别核查，不由在线状态推导信任。
- [x] 原 Server/Embedded key 在进程内作连续性比较，SQLite 状态文件保留；不在摘要输出 key、token 或账号/设备身份。
- [x] 四个进程均 SIGTERM exit 0，HTTP 与两端 gRPC 三监听在每轮关闭后均不可连接；0强杀、0残留进程。数据库账户与政策读取成功说明此 fixture 的持久状态可恢复，不代表任意升级兼容或物理停声。

`health=ok` 仅对临时 noop 配置成立，不能证明真实模型生成、流式语音或 AEC 健康。没有将两轮 smoke 或其多项断言计为新的 unit-test 数。

## Linux 必要测试与计数

两条命令均默认 features、默认测试并行度，未延长测试/生产期限或挑选通过重跑：

```sh
cargo test -p alia-server --lib --test server_runtime --test duplex_playback --locked --offline -j2
cargo test -p alia-embedded --lib --test config --test identity --test binding --test runtime --locked --offline -j2
```

| 命令/目标 | 通过 | 失败 | ignored | 证据 |
| --- | ---: | ---: | ---: | --- |
| Server lib | 313 | 0 | 3 | 含当前 setup Host/Origin/JSON、并发保存、排空及原 body/voice/政策用例 |
| Server duplex_playback | 9 | 0 | 0 | 真实两端协议，三项 Companion 默认并行包含在内；整套25.10秒，不据此关闭V01 |
| Server server_runtime | 53 | 0 | 0 | 真实PG/HTTP、device/health、生命周期及恢复 |
| Embedded lib | 174 | 0 | 0 | 含新增CaptureClock缺失时间/转换重置、真实gRPC可达未绑定/其它控制端拒绝与重启代次；native ALSA边界测试仅模拟/null设备 |
| Embedded binding / config / identity / runtime | 1 / 17 / 1 / 10 | 0 | 0 | SQLite、配置、key和拥有明确所有者的生命周期 |

两条 Cargo 命令分别 exit 0、219.742秒和61.408秒；共 **8个目标、578个不同 Linux 测试通过、0失败、3 ignored**。按每个 Cargo target 的最后一个外层 summary 统计，排除 Server lib 子进程的一个重复通过；summary 保存各测试 binary SHA。原始明细为 `server-runtime-protocol-counts.json` 和 `embedded-runtime-protocol-counts.json`。这些数字不与macOS或历史轮次相加。

本次未运行的3个Linux真实终端测试：

| 完整测试名 | 依赖/状态 |
| --- | --- |
| `bootstrap::setup::tests::setup_terminal_worker_releases_input_after_cancellation` | 真实PTY；本轮Linux未运行 |
| `channels::cli::tests::idle_terminal_worker_joins_and_restores_terminal_on_shutdown` | 真实PTY；本轮Linux未运行 |
| `channels::cli::text::tests::real_console_write_proves_utf8_bytes_and_restores_terminal` | 真实PTY；本轮Linux未运行 |

两个 Easy-Turn live targets 没有选入 Linux 命令，也不记为 Linux 通过或这3个 ignored。macOS 的对应结果单独记录。本轮没有重跑全 Linux workspace。

## 完成 checklist 与资源

- [x] 输入、默认 features、release产物、动态依赖、native库、临时配置与运行命令可追溯。
- [x] 当前 Linux PG/HTTP/gRPC、实际进程重启、必要协议/安全拒绝与生命周期独立实测通过。
- [x] 所有阶段 source drift 为空，最终工作区与快照均匹配633输入，release SHA保持不变。
- [x] 仅删除本轮自有 `linux_release_smoke_v1` schema；任务PG最终非public schema为0、其它连接为0。
- [x] 新Linux容器已停止、Running=false、OOMKilled=false；exit137来自显式停止idle sleep holder，不是Cargo失败。原始日志/缓存/快照保留。
- [x] 独占 Cargo/PG 窗口已归还；Root 再核对任务 schema/连接为 0 后，已停止新任务 PG。旧任务 PG 保持 stopped、用户 5433 容器保持 running，未查询用户数据库；见 [最终资源状态](resource-cleanup.json)。

容器仅能验证非声学软件运行；Pi 现场、硬件型号、AEC、执行器物理反馈和 24 小时整机观察仍按 R02/R03/R05 独立处理。不会由容器结果关闭这些整轮任务。
