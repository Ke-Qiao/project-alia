# Duplex 协议与 Embedded 检查表

本轮从 `codex/duplex-control-protocol` 的 `2619a28` 继续，工作树为 `run/worktrees/alia-duplex-control-protocol`。只拥有 Core/Proto/Embedded；Core `turn.rs` 与 Server 由主代理负责。本文的勾选只表示列出的具体证据完成，不代表暂停纵向集成已经完成。

## DU01：恢复核对

- [x] 读取主仓 `docs/refactor/resume-checkpoint-2026-09-20.md` 与原 `run/duplex-control-handoff/HANDOFF.md`。
- [x] 核对 branch/HEAD=`codex/duplex-control-protocol` / `2619a28fad25dd425d718f0ba9e50a7d55c2fe3b`，开始时工作树干净；没有覆盖其他 agent 的 overlay。
- [x] 检查既有 binding 状态：没有可复用的运行时 generation/revision/身份；现有变化检测依赖配置字段和 `bound_since`。

## DU08-A：原 Binding 与 Ready/Evidence 接纳

- [x] 确定性 red：签名解绑/换绑后的 late Ready 原先仍激活旧 scope；修复后拒绝。
- [x] 确定性 red：旧 Evidence 原先推进替代 scope；相同持久配置重载原先复用旧握手；修复后均拒绝。
- [x] 原 binding 身份与同步接纳共用 Binding 读锁，保持 Binding → Session → backend 的顺序。
- [x] 连接 endpoint 从同一个原绑定快照选择；不持 Binding 锁跨网络、token 文件读取或 native await。
- [x] 原 guard 只按实际 capture owner 清理，不能关闭复用相同 wire scope 的替代 owner。
- [x] 三项真实 loopback 验证 late Ready/旧 Evidence；独立同步 activation gate 确定性验证 Binding 写锁等待。
- [x] 默认与无默认配置的 Embedded 库测试各 153/153 通过，默认配置实际链接锁定 Sherpa 原生库。
- [x] 两种 Embedded all-targets 检查通过，记录实际命令、exit、源码摘要和限制。
- [x] 修复与测试已正常提交 `7bea32152059cce68a2bde89dc79614713d183e8`（`fix: bind voice admission to original binding lifetime`）；仅 7 个 Embedded owned 文件，不 merge/push。

## 后续集成边界

- [ ] 组合 Server ledger/runtime/媒体预算后验证全纵向 Ready/Register/Control/Observe/取消/撤权。
- [ ] 组合后的 workspace / Linux ARM / 真实模型与设备验证；本检查表不预先勾选。

## 2026-09-22 验证记录

实际工作目录为上述工作树，全部命令使用 `--offline --locked`。环境：

```text
CARGO_TARGET_DIR=/Users/ke-qiao/Developer/project-alia/target
SHERPA_ONNX_ARCHIVE_DIR=/Users/ke-qiao/Developer/project-alia/target/sherpa-onnx-prebuilt
DOCS_RS unset
```

| 命令 | 退出码 | 实际结果 | 耗时 |
| --- | --- | --- | --- |
| `cargo test -p alia-embedded --no-default-features --lib module::speech::duplex::tests::binding_admission --offline --locked -- --nocapture`（修复前，真实签名夹具） | 101 | 4 项均按预期失败，保留 `red-signed.log` 与 red source archive | 日志保留编译/测试时间 |
| 同上（完整修复后） | 0 | 8/8，通过含 3 项真实 TCP/gRPC loopback | 编译 6.56s，测试 0.02s |
| `cargo test -p alia-embedded --no-default-features --lib --offline --locked -- --nocapture` | 0 | 153/153，0 ignored | 25.798s |
| `cargo test -p alia-embedded --lib --offline --locked -- --nocapture` | 0 | 153/153，0 ignored | 37.252s |
| `cargo check -p alia-embedded --no-default-features --all-targets --offline --locked` | 0 | 完整目标检查通过 | 11.392s |
| `cargo check -p alia-embedded --all-targets --offline --locked` | 0 | 完整目标检查通过 | 10.242s |
| `git diff --check` 与 7 个变更 Rust 文件的 `rustfmt --edition 2024 --config skip_children=true --check` | 0 | 无空白或格式问题 | — |

本轮原始证据保存在 `run/duplex-ready-validation/`：`validate.py`、`validation.json`、各目标日志，以及 `green-source-before.json` / `green-source-after.json`。两个 manifest 相同；其规范摘要为 `861cf9376d5c9215632fd7b1d7ee59379ef2bfb17d2ade48b8d4672c8d83a916`，覆盖 Cargo 清单/锁、Core、Proto 与 Embedded（包括新增测试）。Sherpa archive `sherpa-onnx-v1.13.1-osx-arm64-static-lib.tar.bz2` 的 SHA-256 为 `ca7baf61f331ea6b848e3630e566647302c26960c7303932bc33125074871eeb`。

实现使用不持久化、clone 保留的 `Arc` 标识原绑定寿命；解绑、重绑、handoff、重载和测试/开发替换在既有 Binding 写锁内使旧身份失效，不引入新持久时钟。收到 Ready 与每条 Evidence 时，在同一 Binding 读锁内核对原身份并同步进入 Session；网络等待与密钥读取在锁外。loopback 直接调用实际 `exchange`，特意不运行 250ms Binding 监视器，证明正确性来自接纳 gate。原 guard 的关闭权限按 capture handle 身份判断，wire UUID 相同也不能越权。

本轮未修改 Core/Proto/Server，playback v3 / voice v2 接口保持不变。没有使用 PG/Docker，也没有启动硬件播放；真实声卡、Pi、声学连续性、完整 Server 语义和端到端恢复仍属于上面的未勾选项。
