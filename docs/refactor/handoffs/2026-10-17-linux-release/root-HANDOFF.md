# Linux 软件发布验收 — Root

2026-10-17；分支 `codex/architecture-refactor`，基线 `da3eca2fe6853f104dd558051aebf92dd62f067b`，应用源码仍为 `4e2968f18d765f326e76d24a3bcdf5bba9034efc`。本轮只补当前软件的 Linux 构建、测试和 release 链接证据；未发现需修复的源码缺口，生产、测试、配置、依赖均未修改。提交仅含本轮文档和验证原件。

## 实际范围与实现检查

先检查既有实现及旧 Linux 证据，再由三个 subagent 分别审计环境/native 输入、测试矩阵、失败与清理边界。旧 `d080710` Server 测试、`d9f9721` Server release、`4434be4` 两端运行证据均不覆盖当前源码。Root 独占本轮 Cargo/数据库窗口，三个分工独立复核原件。

复用既有 Debian 12 / Linux ARM64 / Rust 1.95 镜像与全部离线依赖；未安装新库、扩展平台或启用设备。私有 COW cache/registry 与只读 source/native 防止修改历史 run；Sherpa 官方归档、实际缓存的全部 15 个静态库及系统动态库均核对身份。测试数据库位于专属 internal network，没有发布宿主端口。

保留默认 shadow、当前政策、执行批准、精确取消和资源所有权。`RUST_TEST_THREADS`、`RUST_MIN_STACK`、`DOCS_RS`、`SHERPA_ONNX_LIB_DIR` 均未设置；清除通用 `DATABASE_URL` 后注入任务数据库。`CARGO_BUILD_JOBS=2` 只限制编译，不改变测试并发。原测试期限、CASE permits 和容器内 1800 秒过程 watchdog 保持；宿主 1850 秒仅作失败记录与精确任务容器兜底。各阶段均首次成功，没有失败后重跑、延长测试期限或删除忽略项。

## 验证结果

| 阶段 | 实际结果 | 用时 |
| --- | --- | --- |
| 默认完整 workspace | 1285 passed / 0 failed / 5 既有 ignored / 0 filtered；67 Running + 5 Doc-tests | 483.737 秒 |
| Server no-default all-targets | exit 0 | 21.055 秒 |
| Embedded no-default all-targets | exit 0 | 6.163 秒 |
| 两端 default release | 实际编译、native 链接，exit 0 | 115.452 秒 |
| release ELF/加载器审计 | 两端 AArch64、解释器正确、无缺库、无 RPATH/RUNPATH；Embedded 170 个定义的 Sherpa C API 符号，安全 help exit 0 | 0.405 秒 |

完整测试中 Server library 381/0/3，server_runtime 85/0/0，duplex_playback 31/0/0。相较同源 Mac 1277/0/5，唯一计数变化为 Embedded 库七项 Linux ALSA 软件 `null` 测试及 audio 目标一项 PCM 名称测试；全部 72 目标已对齐，没有缺失 Mac 用例。3 个 nested child 摘要的 3 passed / 1149 filtered 未计入总数；通用计数器 8 个自检通过。5 项 ignored 仍为真实 PTY 3 项、真实 Easy-Turn 2 项，没有新增 ignored。

原 Linux playback child 本轮 build 1016 ms、exit 1376 ms，原 20 秒边界内完成，owner 排空与数据库清理记录完整。它与本轮成功回归均不能关闭历史 V01 未定位根因。

## 源码与产物身份

677 个冻结输入与基线 Git blob、最终宿主源码及上一轮 Mac 受测输入逐项一致。manifest SHA-256：`c239c76799e4f70096c2c7e5898e396534beb2a762fb7628203c671d2941c5bf`。全部六个阶段 source drift 为空；构建后 Sherpa 归档及 15 个实际静态库无变化。

| 产物 | 字节数 | SHA-256 |
| --- | ---: | --- |
| alia-server | 46092832 | `3528c5e02708e126d560cdaade8a431c0ed3e60de6c86892f931dcb0250de330` |
| alia-embedded | 49540200 | `159becdbd3783b77ad308d02096ed39158fc54cfef43f648091a2c0f015f4003` |

两份独立二进制副本在 ignored `run/validation-20261017-linux-release/artifacts/`，不依赖后续 Cargo cache 保持不变。所有原始命令、日志、结果、环境、输入、计数、ELF/动态库哈希和清理记录按原 run 目录结构归档为 50 份文件、721154 字节，见 [索引](evidence-index.json)、[汇总](evidence/evidence/validation-summary.json)、[产物](evidence/evidence/release-audit.json)。脚本依赖原任务路径，不是可在归档目录直接执行的通用部署工具。

## 资源清理与后续出口

测试结束 11.165 秒后，实际同库 SQL 观察为 0 个测试 schema、0 个其他客户端；关闭前再次为 0/0，未自动 DROP 遗留以掩盖问题。Linux holder 仅余 sleep 进程后，按完整 ID 与任务标签回收两个容器、任务 PG 匿名卷和 internal network；共享 astrbot 保持运行，旧任务资源未改动。见 [首次观测](evidence/evidence/after-workspace-resources.json)和[清理](evidence/evidence/cleanup.json)。

本轮仅验收当前 Linux ARM64 的构建、测试、链接和安全加载。Server 没有安全 `--help` 分支，因此未启动 Server 普通进程；Embedded help 在 runtime、模型和设备初始化前返回，未运行 `--check-config`。未以 loader 成功声称服务启停、部署、真实模型或硬件已验收。无源码变更，不重复同源 Mac 测试和格式/架构检查。

下一软件候选为同一 release、已有全 noop staging 下的真实进程启动/关闭/重启、配置与恢复语义验证；先检查模板是否仍符合当前契约，再执行有界演练。实际模型链仍待用户确认端点、版本与运行方式。历史 V01 继续有界跟踪；R02 实物/AEC 资料与声学出口未完成，R03 保持用户暂停，R05 等软件和现场门槛齐备后进入。六轮仍 2/6，不换算项目百分比。
