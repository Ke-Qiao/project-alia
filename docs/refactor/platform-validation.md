# 隔离平台验证

更新：2026-09-23。当前暂停纵向整合的 V2 冻结输入已完成下列软件矩阵；下方 2026-09-20 及更早结果保留其原始范围。不能把不同源码候选的定向测试合并为一次整包结果。

## 暂停纵向整合：V1 失败与 V2 验收

V1 清单 `4e6eadd8d28aeda542e8de0e6dbfbc855cd60d0c83c8c7cdc910702cc6002166`、V2 清单 `1939a948191002882ba70e2578ad87d266d428b2ba423ccfa31aa178a34998d4` 都包含 605 个构建输入。两者只差 Embedded `binding_admission.rs` 和 Server `tests/server_runtime/delivery.rs` 两份夹具，其余 603 项逐字节一致。清单、独立只读快照、原始命令和日志位于整合工作树 `run/validation-20260923/`，最终摘要归档到 [本轮交接](handoffs/2026-09-23/README.md)。

V1 macOS 默认完整 workspace：**964 passed / 3 failed / 5 ignored**，exit 101，520.33 秒，前后无漂移。两项 Embedded 夹具试图重开已退休 epoch，被生产契约拒绝；一项 Runtime panic 夹具没有原 producer/bind，未进入 native 调用。修正保留生产规则和原故障断言，并增加重用旧身份明确拒绝、typed scope retirement 已结束后精确取消 panic 仍保留媒体 owner 的证明。定向绑定 8/8、完整 Runtime 43/43 通过，不另加到最终 workspace 数量。

V1 Linux 的 Core27/Proto4、默认 Server 与两端 no-default all-targets、受影响 Server 目标链接通过；Embedded 完整无默认库为 163/2，复现同两项绑定失败。V1 日志与容器身份保留，容器已移除，没有用修正后的结果覆盖失败。

| V2 检查 | 当前结果 |
| --- | --- |
| macOS 默认完整 workspace | 967 passed / 0 failed / 5 ignored，exit 0，203.85 秒 |
| macOS 两端 no-default all-targets | exit 0，57.30 秒 |
| macOS 变更 Rust 文件格式 | 27 文件，exit 0 |
| macOS 三项真实 PTY | 3 passed / 0 failed；真实 tty、直接 stdout，0.34 秒 |
| macOS 两项真实 Easy-Turn | 2 passed / 0 failed；Easy-Turn 0.59 秒、voice/gRPC/PG 1.71 秒 |
| Linux ARM64 Core / Proto | 27 + 4 passed / 0 failed |
| Linux ARM64 Server 默认与两端 no-default all-targets | exit 0，分别 6.66 / 4.17 秒 |
| Linux ARM64 Embedded 无默认完整库 | 165 passed / 0 failed / 0 ignored，26.46 秒 |
| Linux ARM64 受影响 Server 与真实 PG | Pipeline 21、Voice 23、输出/配对 42、Runtime 43，共 129 passed / 0 failed |

macOS 使用 Rust 1.94.1，默认测试栈/并发，不设置 `RUST_MIN_STACK`、`RUST_TEST_THREADS` 或 `DOCS_RS`。默认 native features 实际链接 Sherpa v1.13.1 osx-arm64 归档，SHA256 `ca7baf61f331ea6b848e3630e566647302c26960c7303932bc33125074871eeb`。构建并发限制为 2，不改变测试并发或行为期限。

V2 Linux 总计 325 个不同测试通过、0 失败、0 ignored，未执行的其它 Server 测试不计入结果；不是 Linux 全 workspace 验收。

Linux 使用固定镜像 `rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1`，实际 Rust 1.95.0 / Debian / aarch64；源码只读挂载，本轮使用独立缓存。只使用任务 PostgreSQL `alia-refactor-test-db:32768`，各 PG 阶段协调窗口；未操作用户 5433 数据库。ALSA 软件 null PCM 与软件 backend 不是 Pi 声卡或实际扬声器证据。

两个 live 目标依次执行，fixture 模型期限保留 30 秒与 8 秒，未额外预热、未重启或清缓存，不宣称冷启动性能。真实 PTY 三项按单线程顺序执行以独占终端；其它常规测试并发未放宽。

全部平台结果仍不代表真实 Pi、AEC、物理 Presented/静音、执行器位置或 24 小时负载验收。终端和实际模型的 5 个 ignored 用例只按其实际运行平台计数。


## Appraisal 整合：macOS 与 Linux ARM64

最终提交候选共 584 个构建输入，SHA-256 manifest 为 `208f11edb3ba7c75e7b3e7acd1042ff002e0c7d150f092d7b495d746c3394cc3`；Cargo.lock 仍为 `0736653b5c57a936dc71d2b18a82b927536acb7bb8c49341f845717c87fde2a2`。Linux 在独立只读快照、私有 target 和固定 ARM64 Rust 镜像中执行，源码与锁文件验证后无漂移。镜像 digest 为 `rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1`，实际平台为 Rust 1.95.0 / Debian / `aarch64-unknown-linux-gnu`，原生 protobuf 3.21.12。真实 PostgreSQL 17.10 使用本任务隔离库，未访问用户 `alia-postgres`。

| 最终 Linux 候选检查 | 实际结果 |
| --- | --- |
| Server 默认所有目标编译 | exit 0 |
| Server 无默认特性所有目标编译 | exit 0 |
| 默认全部测试二进制构建/链接，构建并发 `-j2` | exit 0 |
| 默认 Server 库测试 | 216 通过、3 显式跳过 |
| 全部 45 个 Server 集成目标 | 421 通过、2 显式跳过 |

最终默认测试命令退出 0，约 172 秒，共 **637 个不同测试通过、0 失败、5 跳过**；库中子进程重复项不增加计数。测试运行使用默认并发，没有调整业务或测试期限。5 项需要真实 PTY/模型的测试在 macOS 单独执行通过，未把它们记成 Linux 已执行。

macOS 完整 workspace 候选 `7c1c2e8048e6539ccc555480d3753ba6e0694642043e77acf55f9c6af3c2f9a6` 取得 859 通过、0 失败、5 跳过，之后另跑 PTY 3 项和实际 CPU 模型 2 项通过。最终候选仅将一个 Pipeline 测试夹具的真实构建身份初始化移出行为计时，受影响的默认 Server lib 216/216 重新通过；66 个变更 Rust 文件格式检查和两端无默认特性 all-targets 检查通过。各个 manifest、源码差异和运行后校验分别保存，不将不同候选拼成同一次整包运行。macOS 原始证据为 `run/validation-20260920/appraisal-integration/`。

Linux 原始候选和诊断均保留于 `run/validation-20260920/appraisal-linux/`：

| 候选 | 默认完整测试结果与原因 |
| --- | --- |
| `b8a69cf8…` | 632 通过、4 失败、5 跳过。Context 两项为 PostgreSQL 共享锁容量不足；一项 panic 生命周期测试被额外开启的 backtrace 符号化拖过断言窗口；一项 Pipeline 夹具将首次实际 executable 摘要准备计入 15 秒期限 |
| `01c134af…` | 仅含清除接口原操作身份修复；默认/无默认 check 和全测试二进制构建通过，没有运行 PG 测试 |
| `7c1c2e80…` | 636 通过、1 失败、5 跳过。Context 夹具并发预算修复后 26/26 通过；去掉额外 backtrace 后生命周期通过；Pipeline 初始身份准备仍可独立复现超时 |
| `208f11ed…` | 637 通过、0 失败、5 跳过；全部默认 Server 目标运行完成 |

首次编译还曾由默认高并发触发两次 cgroup OOM kill，发生在测试运行之前；原 exit 101 与内存事件保留。随后仅将构建/链接并发设为 `-j2`，测试本身仍默认并发。Context 夹具将完整 schema 从创建到清理限制为 4 个并行，用例内部 SQL 竞争及 45 秒期限未变。

Pipeline 诊断在同一 ELF、无 backtrace、独占测试数据库时仍失败。`/proc` 中实际 executable 读取位置连续约 14.2 秒从 3,342,336 增至 413,597,696 字节，文件共 415,129,944 字节；对应现有 `ExecutionService::new → build_digest` 的真实 SHA-256 初始化。最终修正使用同一真实入口完成夹具准备，没有伪造构建身份或延长 15 秒行为期限。最终测试无新 schema/连接残留；此前本任务失败留下的已确认 schema 单独清理，不改其他基线测试数据。全部候选容器按任务身份清理，源快照和证据保留。

以上覆盖已实现的 Server 来源、投影、执行与投递契约，不证明可恢复暂停的网络启用、Pi 音频呈现、AEC、实际机构到位或跨架构模型性能。

## 音频参考包：macOS 与 Linux ARM64

`5467e8b` 已通过 macOS 默认特性实际编译/链接和 124 项 Embedded 库测试；无默认特性 all-targets 检查与 124 项库测试也通过。使用上游支持的本地 Sherpa v1.13.1 arm64 原生归档，归档 SHA-256 为 `ca7baf61f331ea6b848e3630e566647302c26960c7303932bc33125074871eeb`，没有跳过原生链接。冻结后参考/关闭专项 7 项通过。

Linux 使用下文相同的固定 ARM64 Rust 镜像，597 个构建输入只读挂载，清单 SHA-256 为 `503f32ba40b55419ac5f7bc320b6900d85bd19071d6e426426018d99a2ee8d53`。Embedded 无默认特性 all-targets 检查通过，库测试 130 项全部通过。源码冻结后增加的验证说明文档不属于构建清单；Cargo.lock 未改变。最终环境、依赖版本、日志及退出码保存在 `run/recovery-20260920/audio/validation/evidence-v2/`，逐文件哈希和结果汇总为 `run/recovery-20260920/audio/FINAL.json`。任务容器退出后已确认移除。

ALSA 的 6 项测试分别使用真实 libasound 软件 null PCM 和显式注入的故障结果验证原生调用及共用错误处理入口；null 不具有硬件采样节奏，不能证明实机 wait/断连或声学回声行为。本轮没有新增 Linux 默认 Sherpa 运行时或 Server 全套验证，不能用音频子包的通过数覆盖这些范围。实现与剩余 AEC/漂移映射工作见[音频参考契约](audio-reference-clock-design.md)。

## 历史证据可用性

2026-09-20 恢复检查发现，先前 `/private/tmp` 中的源码归档和完整日志已不完整。可从本任务记录恢复的 Context 原始报告、诊断、计数摘要、夹具补丁及 67 项候选结果保存在 `run/recovery-20260920/evidence/body-server/`，其中 README 列明来源与缺失项。完整原平台日志、归档、manifest 原件和二进制未能恢复，不宣称这些原件仍可打开。下方保留当时实际执行的结果与原位置供追踪；新的变更使用持久 `run/` 目录重新验证，不沿用历史通过数作为当前整包证据。

## Context：固定提交 a33ec67 与夹具修正复核

从 `a33ec67f9e4a0916ceac68fed4c4dc4f476f98bd` 导出 594 个文件到 `/private/tmp/alia-platform-a33ec67f.5n5idovv/source`，只读挂载执行。源码在完整矩阵、串行复核与只读诊断前后完全一致。目标仍为 Debian 12 / Rust 1.95.0 / `aarch64-unknown-linux-gnu`，使用独立 COW 缓存；测试数据库是新建 PostgreSQL 17.10 / pgvector 容器、独立网络与 tmpfs，无宿主发布端口。

| 输入 | 实际值 |
| --- | --- |
| 官方 Rust 镜像 | `rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1` |
| ARM64 image ID | `sha256:7cf1e580ef5539f03b58560753e8ab84c8c360960d99dff714004aa98f203977` |
| Cargo.lock SHA256 | `0736653b5c57a936dc71d2b18a82b927536acb7bb8c49341f845717c87fde2a2` |
| 源码清单 SHA256 | `bbbc73a7a4c021716a7add70b40d12f2ba9569cf0bc5ebe0b0d6bc4e1dc9b41c` |
| 源码 archive SHA256 | `dab580a93fa5b08159e1f4ecd709b74c8f1915ca1bc2207de588f86c27429936` |

| 固定提交的验证范围 | 实际结果 | 墙钟耗时 |
| --- | --- | --- |
| Core | 20 passed | 0.68 s |
| Server 默认 all-targets check | exit 0 | 18.07 s |
| Server 默认库测试 | 218 passed / 3 ignored | 57.28 s |
| 全部 41 个 Server 集成 targets | 357 passed / 4 failed / 2 ignored | 205.18 s |
| Embedded 默认完整 build/link | exit 0 | 9.82 s |
| Embedded 默认库测试 | 108 passed | 32.39 s |

原提交共 **703 unique passed、4 failed、5 ignored**，库内子进程重复项不加计。三项 Execution 失败为 `lost_approval_commit_ack_keeps_session_cancellation_ownership`、`approval_commit_ack_timeout_keeps_session_cancellation_ownership`、`dropped_submit_after_approval_commit_keeps_session_cancellation_ownership`；均在等待故障注入代理确认第二个 COMMIT 的 3 秒门槛处失败。Runtime 的 `cancelling_or_dropping_build_future_rolls_back_partial_init` 在等待测试模块 initialized 的 5 秒门槛处失败。四项同源单独/串行复核仍然失败，不能将其解释为只发生于并行争用。

只读 PG statement timing 与 backtrace 诊断表明，Runtime 已结束迁移和工具登记，随后首次 `ExecutionService::new` 需要对实际 debug 测试二进制计算构建身份；该开销进入了本应等待指定模块的测试门槛。Execution 的 SQL 本身较快，但代理两端未启用 TCP_NODELAY，且每个回复帧的 header/body 分成两次小写入；代理连接查询间隔中位数约 44 ms，与 Nagle/延迟 ACK 放大相符。保留具体时间与推断边界于 `evidence/diagnosis.md`。

随后另建 `/private/tmp/alia-platform-a33-fixture.a5ecbj__/source`，仅应用两测试文件的修正：代理两端启用 TCP_NODELAY 并一次写完整帧；取消测试在开始等待模块前，通过真实 `ExecutionService::new` 预热真实二进制身份。没有伪造身份，没有更改生产代码、3 秒 COMMIT 观察期限或 5 秒取消/模块观察期限。594 个文件中其余 592 个及 Cargo.lock 与原归档完全一致。

| 候选证据 | 实际值 |
| --- | --- |
| 夹具补丁 SHA256 | `2254c86c1260d22b17c3f6beb9b87caafd14e79bb08d35a600d64b9e89eaf83d` |
| 候选源码清单 SHA256 | `f9f9bc10558de421d64a289915a756d6090005c57f76f3cdb9aa7a07831eee8d` |
| 原四项定向 | 4 passed；不再加计到下列总数 |
| 完整 Execution / Runtime | 30 / 37 passed，0 failed，0 ignored |
| 并行条件 | 两进程于 2026-09-13 16:19:10 UTC 同时开始，各自保留默认内部并行 |
| Execution / Runtime 耗时 | 32.17 s / 60.86 s |

候选合计 **67 unique passed**，源码前后摘要相同。它证明这两处夹具修正在本轮环境解决上述失败；它不是对固定 `a33ec67` 重新给出全绿结果，也没有覆盖正在开发的 Memory 向量或 Appraisal 代码。正式主线采用相同夹具修正，格式整理不改变行为。

两轮各自的两个容器和一个网络均按其 ID/任务标签清理完毕，临时 PG 数据随 tmpfs 移除。没有修改用户 5433 数据库、任务常用 32768 数据库或其他容器。源码、独立缓存与 evidence 保留。完整入口为 `/private/tmp/alia-platform-a33ec67f.5n5idovv/evidence/REPORT.md`；各轮的 `result-summary.json` / `candidate-result-summary.json` 保存逐阶段命令、时间、退出码、哈希和清理证据。

默认 native features 与 Sherpa 链接保留，没有设置 DOCS_RS 或用 no-default 替代。5 项 ignored 为 3 项真实 PTY、2 项真实 Easy-Turn/gRPC，Linux 本轮未执行。没有真实 Pi、声卡/AEC、双讲、物理动作反馈、实际 LLM/TTS 或 24 小时负载证据。

## TextDelivery / D3：固定提交 e63d40c

从 `e63d40ccac69cfb0162921e2b535737fd6c5ecec` 导出 **579 个文件**到 `/private/tmp/alia-platform-next.wMxQXW/source`，只读挂载为 `/src`。独立缓存由上一轮缓存 APFS 克隆，897 个克隆文件逐项 SHA256 一致；已清理的 registry / Sherpa 归档按 Cargo.lock 和固定公开发行版重新取得。三次运行前后源码清单、源码文件与 Cargo.lock 均完全一致，没有以正在实施的 ContextLease 工作区替换输入。

| 输入 | 实际值 |
| --- | --- |
| 官方 Rust 镜像 | `rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1` |
| ARM64 image ID | `sha256:7cf1e580ef5539f03b58560753e8ab84c8c360960d99dff714004aa98f203977` |
| Rust / Cargo / target | `1.95.0` / `1.95.0` / `aarch64-unknown-linux-gnu`，Debian 12 |
| Cargo.lock SHA256 | `0736653b5c57a936dc71d2b18a82b927536acb7bb8c49341f845717c87fde2a2` |
| 源码清单 SHA256 | `54e822a2cf77b5785284d268c7bf1ff015012fb688a4466355e2924fc995a3f6` |
| 数据库 | 任务容器 `alia-refactor-test-db` 的独立库 `alia_platform_e63d40c`，端口 32768，PostgreSQL 17 / pgvector 0.8.2 |
| Sherpa 静态归档 | `1.13.1` Linux aarch64，SHA256 `86d0d3818770d4f6a087e52691d3ae171393972030f133df064a271fe9fe1749` |

实际验证保留了两次失败及复核过程。第一次遗漏系统包 `libprotobuf-dev`，Server 编译找不到 `google/protobuf/empty.proto`；第二次补齐该包，Core、Server 全 targets 检查和库测试通过。第二次 Server 集成测试中，`cancelling_or_dropping_build_future_rolls_back_partial_init` 在等待测试同步信号的 5 秒 deadline 处超时，Runtime 同组另外 32 项通过。第三次仅复核该 Runtime 目标并补跑尚未执行的目标，使用相同源码、锁文件和默认 features；Runtime 以 `--test-threads=1` 执行 33 项全部通过。**记录为并行执行时的测试时序敏感性，不能据串行通过宣称并行稳定性已得到证明。**

| 验证范围 | 最终覆盖 | 墙钟耗时 |
| --- | --- | --- |
| Core 库契约 | 20 passed | 0.14 s |
| Server 默认 `--all-targets --locked` 检查 | exit 0 | 17.40 s |
| Server 默认库测试 | 202 passed / 3 ignored | 59.61 s |
| Server 全部 39 个集成 targets | 312 unique passed / 2 ignored；包含上述串行复核 | 首次 202.11 s；Runtime 复核 92.39 s；余下 4 targets 0.27 s |
| Embedded 默认完整 build/link | exit 0，保留 Sherpa、STT、direct-response | 27.06 s |
| Embedded 默认库测试 | 108 passed | 45.56 s（测试执行 26.23 s） |

完整覆盖合计 **642 unique passed、5 ignored**：Server 514、Embedded 108、Core 20。库内子进程重复测试及 Runtime 重跑不重复累计。五项 ignored 为三项真实 PTY 库测试和两个真实 Easy-Turn 集成目标，本轮未在 Linux 执行，不能将宿主 macOS 的验证结果算入。首轮和第二轮的非零退出状态原样保留。

主要阶段时间为 2026-09-13 14:28:33–14:38:24 UTC（北京时间 22:28:33–22:38:24），中间包含故障分析和分阶段复核。完整 22 项迁移在随机 schema 中执行，包含 TextDelivery `000005` 和 BodyDelivery `000006`；最终测试库仅剩系统 schema 和 public。`alia-platform-e63d40c-wmxqxw` 与 `alia-platform-e63d40c-continuation` 已退出，查询确认均已自动移除。任务 PostgreSQL 保留，未操作用户 5433 数据库。

Embedded 产物是 ARM aarch64 ELF 64-bit PIE，解释器 `/lib/ld-linux-aarch64.so.1`，BuildID `18aba3f82bacb89b4c5132a92d1e818b1ca1894b`，SHA256 `864a389f3bdf6c73dc08eabd98f41abfd53be2ab14306b3c10c6c3ed21a50d47`。没有通过禁用默认 features 或设置 DOCS_RS 绕过 native 链接。编译与测试使用克隆缓存，不是冷缓存性能测量。

原始证据在 `/private/tmp/alia-platform-next.wMxQXW/evidence`，包含 `attempt-2`、`attempt-3` 各阶段 command/start/end/status/log、系统依赖、数据库前后状态和逐文件摘要。合并计数在 `combined-result-summary.json`；两个实际脚本为 `attempt-2/run.sh`、`attempt-3/run.sh`。第三次脚本明确只运行 Runtime 串行复核、`settings` / `skills` / `tools` / `voice_stream_live` 四个剩余集成目标以及 Embedded build/lib。

本轮没有执行 Embedded 集成测试、真实 Pi 5、声卡、AEC、双讲或物理动作反馈，也没有执行真实 Easy-Turn / LLM / TTS。该结果覆盖 `e63d40c` 的 TextDelivery 与签名 Body 控制，**不覆盖后续 ContextLease / Cache 改动**，不证明其他操作系统或 CPU 架构已验收。

## D1/D2：固定提交 ee9cbfd

本轮从 `ee9cbfd4f8b2c4c2fbe8c8a1c985be7fddb2d38b` 导出 **555 个文件**至 `/private/tmp/alia-platform-ee9cbfd.hKIL5z/source`，只读挂载 `/src`。使用 APFS `cp -cR` 从 `/private/tmp/alia-platform-2f8797c.QL6Y8V/cache` 克隆独立缓存至本轮 `cache`，没有修改旧缓存、归档源码或工作区 Cargo.lock。所有 555 个归档文件的开始/结束 SHA256 完全一致。

| 输入或环境 | 本轮实际值 |
| --- | --- |
| 官方 Rust 镜像 | `rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1` |
| ARM64 image ID | `sha256:7cf1e580ef5539f03b58560753e8ab84c8c360960d99dff714004aa98f203977` |
| Rust / Cargo / LLVM | Rust `1.95.0 (59807616e 2026-04-14)`、Cargo `1.95.0 (f2d3ce0bd 2026-03-21)`、LLVM `22.1.2` |
| 系统 | Debian 12，`aarch64-unknown-linux-gnu`，OrbStack kernel `7.0.14-orbstack-00380-ga7e0a2dc9535` |
| Cargo.lock SHA256 | `475d4f5b42bcfde3f9194d9040ed52a513ecc9ad47271560beaea220b6b98ea9`，前后相同 |
| 资源限制 | 6 CPU、12 GiB、512 PIDs，`CARGO_BUILD_JOBS=4` |
| 任务容器 | `alia-platform-ee9cbfd-hkil5z`，`--rm` 结束后确认移除 |
| 任务数据库 | 已授权 `alia-refactor-test-db` 容器中新建空库 `alia_platform_ee9cbfd`，账号 `alia_test`，容器访问 `host.docker.internal:32768` |
| PostgreSQL / 扩展 | PostgreSQL `17.10`，aarch64；`vector 0.8.2`；测试结束 `pgcrypto 1.3`、`plpgsql 1.0` 同时存在 |

系统依赖实际包含 protobuf `3.21.12-3+deb12u1`、Clang `1:14.0-55.7~deb12u1`、CMake `3.25.1-1`、ALSA `1.2.8-1+b1`、DBus `1.14.10-1~deb12u1`、OpenSSL `3.0.20-1~deb12u2` 和 PostgreSQL client `15.19-0+deb12u1`，完整包版本见 evidence/system-packages.txt。没有挂载宿主 Cargo 目录或 Docker socket，也没有操作用户 `alia-postgres`（5433）、模型服务或其他用户容器。

### 实际结果与计数

| 检查 | 实际命令 | 结果 | 墙钟耗时 |
| --- | --- | --- | --- |
| Server 默认 features，所有 targets | `cargo check -p alia-server --all-targets --locked` | exit 0 | 17.27 s |
| Server 默认库测试 | `cargo test -p alia-server --lib --locked` | 176 passed / 0 failed / 2 ignored | 36.87 s，测试执行 0.49 s |
| Server 全部 36 个集成 targets | 下方完整命令 | 296 passed / 0 failed / 2 ignored | 190.39 s |
| Embedded 默认 Sherpa 完整构建 | `cargo build -p alia-embedded --locked` | 编译与最终链接 exit 0 | 16.23 s |
| Embedded 默认库测试 | `cargo test -p alia-embedded --lib --locked` | 70 passed / 0 failed / 0 ignored | 40.72 s，测试执行 25.75 s |

五阶段时间为 **2026-09-08 07:41:49–07:46:50 UTC**，北京时间 **15:41:49–15:46:50**。共 **542 项 unique passed、0 failed、4 ignored**：Server 472 项，Embedded 70 项。Server 库测试内子进程再次执行的一项已包含在 176 中，不重复累计。复制缓存的耗时不是冷缓存性能基准；所有阶段使用默认 features，没有以 no-default 替代。

编译没有 Rust warning。Embedded 库测试输出一条 ONNX Runtime `Unknown CPU vendor` 警告；测试仍全部通过。此处记录真实 native 初始化输出，不把它描述为模型推理或真实音频验收。

重点集成目标如下，其余目标同属上表 296 项，完整逐目标计数见 evidence/result-summary.json：

| 目标 | passed | 测试执行耗时 |
| --- | ---: | ---: |
| `delivery_repository` | 14 | 1.61 s |
| `delivery_service` | 12 | 10.62 s |
| `execution_repository` | 19 | 2.48 s |
| `execution_service` | 27 | 34.09 s |
| `memory` / `memory_privacy` | 13 / 24 | 0.16 / 3.73 s |
| `migrations` | 7 | 0.41 s |
| `server_runtime` | 32 | 61.29 s |
| `ingress_identity` | 8 | 0.01 s |
| `tools` | 15 | 0.01 s |

Delivery 的并发幂等、单次 dispatch、取消、恢复 Unknown 和 native owner 保留在真实 PostgreSQL/运行时测试中通过。Memory/Execution 的删除后旧来源、快慢钟、当前 source epoch、跨 owner、事务故障与旧 embedding 回填回归均实际执行。模型和工具使用受控替身，数据库与网络 loopback 为真实环境。

Embedded 库测试实际包含本轮 D2 的 CPAL 在途完整帧取消 barrier、ALSA 部分提交取消 barrier、End ACK 等待输入租约释放、等待 Start 撤权、排队 begin 撤权、响应断开后的 native enqueue join、同 epoch 精确取消保留后续 FIFO、重采样相位/尾部/频谱等用例。它们验证生产控制及计数逻辑在 ARM64 的执行，没有开启真实声卡。

### 扩展、迁移与链接证据

测试库初始为空，预先在 `public` 安装 pgvector。归档内 manifest 包含 **20 个 SQL 迁移**，包括 Execution、Memory privacy、source privacy epoch 和 Delivery 的 `m20260908_000001` 至 `000004`。`bundled_schema_installs_with_pgvector_and_restarts_without_changes` 真实执行完整迁移，验证重复启动不再执行，以及 pgvector 属于 public；其余迁移测试覆盖并发只应用一次、SQL 和 journal 回滚、已应用 SQL 改变被拒绝与带分号的函数体。

各真实 PG fixture 创建并清理独立随机 schema。最后查询数据库只剩 `information_schema`、`pg_catalog`、`pg_toast`、`public`，没有遗留测试 schema；任务数据库与 PostgreSQL 容器保留运行。完整迁移文件 SHA256 记录在 evidence/migration-manifest-sha256.txt。

Embedded 默认 `wakeword-sherpa`、`stt-sherpa`、`direct-response` 保留。产物 `/private/tmp/alia-platform-ee9cbfd.hKIL5z/cache/target/debug/alia-embedded` 是 ARM aarch64 ELF 64-bit PIE，解释器 `/lib/ld-linux-aarch64.so.1`，BuildID `b196b86ceef9a1d8a0f8fb311b30c13530616fb3`，SHA256 `ea025bf856aa5363a2e97a9273554e0f7e354528631e52f7f2fea879ffb816c1`。官方 Sherpa `1.13.1` Linux aarch64 静态归档 SHA256 仍为 `86d0d3818770d4f6a087e52691d3ae171393972030f133df064a271fe9fe1749`。没有设置 DOCS_RS、禁用 Sherpa 或下载模型。

### 复现与排除项

实际脚本：`/private/tmp/alia-platform-ee9cbfd.hKIL5z/evidence/run.sh`。完整 Docker 命令使用下列隔离参数；启动后脚本安装与上方相同的系统依赖，数据库预检失败会立即终止，各 Cargo 阶段使用 `/usr/bin/time -v timeout 2400`：

```bash
docker run --rm --name alia-platform-ee9cbfd-hkil5z --platform linux/arm64 \
  --cpus 6 --memory 12g --pids-limit 512 \
  --mount type=bind,source=/private/tmp/alia-platform-ee9cbfd.hKIL5z/source,target=/src,readonly \
  --mount type=bind,source=/private/tmp/alia-platform-ee9cbfd.hKIL5z/cache,target=/cache \
  --mount type=bind,source=/private/tmp/alia-platform-ee9cbfd.hKIL5z/evidence,target=/evidence \
  --env CARGO_HOME=/cache/cargo --env CARGO_TARGET_DIR=/cache/target \
  --env CARGO_BUILD_JOBS=4 --env DEBIAN_FRONTEND=noninteractive \
  --env ALIA_TEST_DATABASE_URL=postgresql://alia_test:alia_test@host.docker.internal:32768/alia_platform_ee9cbfd \
  --workdir /src \
  rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1 \
  bash /evidence/run.sh
```

脚本中实际的全部集成测试命令：

```bash
cargo test -p alia-server --test channel_registry --test config \
  --test delivery_repository --test delivery_service --test easy_turn_live \
  --test embedded_channel --test execution_repository --test execution_service \
  --test expression_abi --test expression_appraisal --test expression_eval \
  --test expression_regulation --test expression_trace --test ingress_identity \
  --test llm --test llm_providers --test memory --test memory_capture \
  --test memory_privacy --test migrations --test persona --test pipeline \
  --test pipeline_tools --test plugin_storage --test provider_adapters \
  --test registry --test router --test rules --test scheduler --test sensors \
  --test server_background --test server_runtime --test settings --test skills \
  --test tools --test voice_stream_live --locked
```

原始 evidence 包含镜像 ID、环境/依赖、数据库前后检查、五阶段 command/start/end/status/log、测试汇总、迁移摘要、全部源码前后摘要、链接产物摘要以及容器退出确认。四项 ignored 是两项真实 PTY 库测试及 `easy_turn_live`、`voice_stream_live` 各一项，未计入通过。宿主另跑的 PTY/模型结果不能计作本轮 Linux 结果。

本轮没有执行完整 workspace、Embedded 集成测试或设备二进制；没有真实 Easy-Turn/LLM/TTS、声卡/ALSA 设备、Pi 5、蓝牙、AEC、双讲或物理呈现验收。它不证明 Linux x86_64、Windows、macOS Intel，也不覆盖 `ee9cbfd` 后的 ContextLease 或 D3 身体动作实施。

## Runtime 与来源 epoch：固定提交 2f8797c

源码从 `2f8797c7b1ce73f62e8abb798820bb5bdbdf1cbe` 导出到 `/private/tmp/alia-platform-2f8797c.QL6Y8V/source`，容器只读挂载为 `/src`。以 APFS `cp -cR` 克隆前次 `/private/tmp/alia-platform-5aecb38.rTx8mP/cache` 到本轮独立缓存；没有覆盖旧缓存或挂载正在变化的工作区、宿主 Cargo 目录、Docker socket。结束时核验全部 **536 个归档源码文件 SHA256 均未变化**。

| 输入或环境 | 本次实际值 |
| --- | --- |
| 镜像固定摘要 | `rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1` |
| ARM64 image ID | `sha256:7cf1e580ef5539f03b58560753e8ab84c8c360960d99dff714004aa98f203977` |
| Cargo.lock SHA256 | `76f193b1b04e7d310c6bded15da0fb1281465fe4c7ff276db519928377415d67`，开始/结束相同 |
| Rust / Cargo | `rustc 1.95.0 (59807616e 2026-04-14)` / `cargo 1.95.0 (f2d3ce0bd 2026-03-21)` |
| Rust host / LLVM | `aarch64-unknown-linux-gnu` / `22.1.2` |
| 系统 | Debian 12，Docker Engine 29.4.0，OrbStack kernel `7.0.14-orbstack-00380-ga7e0a2dc9535` |
| 隔离限制 | 6 CPU、12 GiB、512 PIDs，`CARGO_BUILD_JOBS=4` |
| 任务容器 | `alia-platform-2f8797c-ql6y8v`，`--rm` 退出后确认已移除 |
| 真实数据库 | `host.docker.internal:32768/alia_refactor_runtime_v1`，专用账号 `alia_test`；PostgreSQL `17.10 (Debian 17.10-1.pgdg12+1)`，aarch64 |

系统构建依赖及实际版本与下方 `5aecb38` / `ef07d43` 环境记录相同，包含 `libprotobuf-dev`、ALSA、DBus、OpenSSL、Clang、CMake 和 PostgreSQL 客户端。`SELECT current_database(), version()` 预检成功；各真实数据库测试创建、迁移并清理自身随机 schema。没有改动用户 `alia-postgres`（5433）或其他已有容器；专用测试 PostgreSQL 保持运行，未操作模型服务。

### 实际结果

| 检查 | 命令 | 结果 | 墙钟耗时 |
| --- | --- | --- | --- |
| Server 默认 features，所有 targets | `cargo check -p alia-server --all-targets --locked` | exit 0，无警告 | 17.47 s |
| Server 默认 features，库测试 | `cargo test -p alia-server --lib --locked` | 161 passed / 0 failed / 2 ignored | 37.46 s，测试执行 0.38 s |
| Memory、来源 epoch、Execution、runtime 集成 | 下方完整命令 | 119 passed / 0 failed / 0 ignored | 129.12 s，含编译 39.88 s |
| Embedded 默认 Sherpa，完整构建 | `cargo build -p alia-embedded --locked` | 编译及最终链接通过，无警告 | 18.40 s |

四阶段为 **2026-09-08 05:29:59–05:33:22 UTC**（北京时间 **13:29:59–13:33:22**）。使用复制缓存，不是冷缓存性能基准。全部 Cargo 命令使用默认 features 和 `--locked`，未用 `--no-default-features` 替代。库测试中子进程重跑的一项测试已包含在 161 中，不重复累计。

集成命令与各目标的实际数量如下：

```bash
cargo test -p alia-server --test memory_privacy --test memory --test tools \
  --test execution_repository --test execution_service --test server_runtime --locked
```

| 目标 | passed | 测试执行耗时 |
| --- | ---: | ---: |
| `execution_repository` | 19 | 2.28 s |
| `execution_service` | 27 | 32.99 s |
| `memory` | 13 | 0.13 s |
| `memory_privacy` | 24 | 3.60 s |
| `server_runtime` | 21 | 50.16 s |
| `tools` | 15 | 0.01 s |

因此，本次 Server 库及选定集成目标共 **280 项通过**。真实 SQL 覆盖原子批量写入/遗忘、SQL 故障回滚、跨 owner 隔离、源 revision 变化、删除与派生提交竞态、晚返回 embedding、来源接纳锁的 2 秒上限及当前账户验证。Conversation 库测试证明旧 epoch 输入晚落库仍保留原文，但不能写回 Memory；新 epoch 输入可继续记忆。Execution 在正常、超前 60 秒和落后 60 秒的来源时间戳下，旧来源清除后均不能写回，新来源均成功；偏移注入的是测试来源时间戳，没有调整宿主或数据库时钟。Insights、timer、subscription、rule 的真实调用链也通过，涵盖旧/NULL epoch 来源排除、快慢钟新来源、慢条件等待和取消/关闭。LLM 与工具外部行为由测试替身控制，数据库、锁、事务、runtime 和 loopback 网络实际执行。

Embedded 默认 `wakeword-sherpa`、`stt-sherpa`、`direct-response` 均保留；本轮重新编译工作区 crate 并完成最终链接。产物 `/private/tmp/alia-platform-2f8797c.QL6Y8V/cache/target/debug/alia-embedded` 为 ARM aarch64 ELF 64-bit PIE，解释器 `/lib/ld-linux-aarch64.so.1`，BuildID `3fbffac7c31f50840e604f9a53cddfb6a1d4e4c3`，SHA256 `803c8d53fdc5f1221e1cdc10198d0e0458f56c4a3de20361c5b3004ac00805bc`。缓存中的官方 Sherpa `1.13.1` Linux aarch64 静态归档 SHA256 再次确认为 `86d0d3818770d4f6a087e52691d3ae171393972030f133df064a271fe9fe1749`；未设置 `DOCS_RS`、禁用 Sherpa 或下载模型。

### 复现、证据和排除项

下列命令使用已授权的专用测试库。数据库不可达时测试必须失败，不得换成用户数据库或把跳过计作通过。

```bash
validation_dir=$(mktemp -d /private/tmp/alia-platform-2f8797c.XXXXXX)
mkdir -p "$validation_dir/source"
git archive 2f8797c7b1ce73f62e8abb798820bb5bdbdf1cbe | tar -x -C "$validation_dir/source"
cp -cR /private/tmp/alia-platform-5aecb38.rTx8mP/cache "$validation_dir/cache"
docker run --rm --platform linux/arm64 --cpus 6 --memory 12g --pids-limit 512 \
  --mount "type=bind,source=$validation_dir/source,target=/src,readonly" \
  --mount "type=bind,source=$validation_dir/cache,target=/cache" \
  --env CARGO_HOME=/cache/cargo --env CARGO_TARGET_DIR=/cache/target \
  --env CARGO_BUILD_JOBS=4 --env DEBIAN_FRONTEND=noninteractive \
  --env ALIA_TEST_DATABASE_URL=postgresql://alia_test:alia_test@host.docker.internal:32768/alia_refactor_runtime_v1 \
  --workdir /src \
  rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1 \
  bash -eu -c '
    apt-get update
    apt-get install --no-install-recommends -y ca-certificates pkg-config protobuf-compiler libprotobuf-dev libssl-dev libasound2-dev libdbus-1-dev cmake clang time postgresql-client
    rustc -vV
    cargo -V
    sha256sum Cargo.lock
    psql -X -v ON_ERROR_STOP=1 "$ALIA_TEST_DATABASE_URL" -c "SELECT current_database(), version();"
    cargo check -p alia-server --all-targets --locked
    cargo test -p alia-server --lib --locked
    cargo test -p alia-server --test memory_privacy --test memory --test tools --test execution_repository --test execution_service --test server_runtime --locked
    cargo build -p alia-embedded --locked
    sha256sum Cargo.lock
  '
```

实际脚本与原始证据保存在 `/private/tmp/alia-platform-2f8797c.QL6Y8V/evidence/`：`run.sh`、固定 SHA、镜像/环境/系统包信息、数据库预检、各阶段 `.command/.start/.end/.status/.log`、完整源码摘要、Cargo.lock 结束摘要、Embedded 文件类型/摘要、Sherpa 归档摘要及容器退出确认。每阶段独立限时 2400 秒，使用 `/usr/bin/time -v`，四个退出码均为 0。

本次两项 ignored 是 `setup_terminal_worker_releases_input_after_cancellation` 与 `idle_terminal_worker_joins_and_restores_terminal_on_shutdown`，要求真实 PTY，未计入通过。没有运行其他 Server 集成目标、完整 workspace 测试、Embedded 单测或设备二进制；真实 Easy-Turn/LLM/TTS、Pi 5 音频/蓝牙/AEC/双讲没有在本容器验证。其他环境另跑的 PTY/模型结果不能算作本次 Linux 结果。此证据不证明 Linux x86_64、Windows、macOS Intel，也不覆盖 `2f8797c` 后的 Delivery 修改。

## P2：固定提交 5aecb38

本次将 `5aecb38cc9bceb51f1b424c99f6e76f944f0a544` 导出到 `/private/tmp/alia-platform-5aecb38.rTx8mP/source` 并只读挂载为 `/src`。上次 `/private/tmp/alia-platform-ef07d43.maWX2V/cache` 通过 macOS APFS `cp -cR` 复制到本任务独立 `cache`；后续构建没有覆盖旧缓存或旧产物，也没有挂载宿主 Cargo 缓存、工作区源码或 Docker socket。

环境保持官方 `rust:1.95-bookworm` 的 Linux ARM64 镜像：

| 输入或环境 | 本次实际值 |
| --- | --- |
| 镜像固定摘要 | `rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1` |
| ARM64 image ID | `sha256:7cf1e580ef5539f03b58560753e8ab84c8c360960d99dff714004aa98f203977` |
| Cargo.lock SHA256 | `f602172bab2485216d75eeb6bfffd0b5321cefadd12b08ec39d92fdc3e7e129e`，开始/结束相同 |
| Rust / Cargo | `rustc 1.95.0 (59807616e 2026-04-14)` / `cargo 1.95.0 (f2d3ce0bd 2026-03-21)` |
| Rust host / LLVM | `aarch64-unknown-linux-gnu` / `22.1.2` |
| 系统 | Debian 12，Docker Engine 29.4.0，OrbStack kernel `7.0.14-orbstack-00380-ga7e0a2dc9535` |
| 隔离限制 | 6 CPU、12 GiB、512 PIDs，`CARGO_BUILD_JOBS=4` |
| 任务容器 | `alia-platform-5aecb38-rtx8mp`，`--rm` 退出后已确认移除 |
| 真实数据库 | `host.docker.internal:32768/alia_refactor_p2_verify2`，专用测试账号 `alia_test`；PostgreSQL `17.10 (Debian 17.10-1.pgdg12+1)`，aarch64 |

容器安装了下方历史环境表中的同版本系统构建依赖，额外增加数据库探测客户端 `postgresql-client 15+248+deb12u1` 和 `postgresql-client-15 15.19-0+deb12u1`。真实 SQL `SELECT current_database(), version()` 已成功确认库名；没有连接用户的 5433 端口或操作用户 `alia-postgres`。库测试通过 `ALIA_TEST_DATABASE_URL` 连接该专用实例，创建和删除自身随机 schema；数据库缺失会导致测试明确失败，不会跳过。

### P2 实际结果

| 检查 | 命令 | 实际结果 | 墙钟耗时 |
| --- | --- | --- | --- |
| Server 默认 features，所有 targets | `cargo check -p alia-server --all-targets --locked` | exit 0，无警告 | 17.78 s |
| Server 默认 features，库测试 | `cargo test -p alia-server --lib --locked` | exit 0；152 passed / 0 failed / 2 ignored | 37.52 s，测试执行 0.35 s |
| Embedded 默认 Sherpa features，完整构建 | `cargo build -p alia-embedded --locked` | exit 0，编译及最终链接通过，无警告 | 15.07 s |

三阶段实际时间为 2026-09-08 **02:59:30–03:00:40 UTC**（北京时间 **10:59:30–11:00:40**）。这些耗时使用复制的现有缓存，不是全新缓存基准。未修改代码、Cargo.lock 或任何 feature 选择；Embedded 默认仍是 `wakeword-sherpa`、`stt-sherpa`、`direct-response`。

本次库测试中的 `providers::conversation::database::tests::durable_input_provenance_preserves_time_owner_and_atomicity` 在真实 PostgreSQL 上通过，覆盖实际来源行 UUID、原始接收时间、owner 校验和消息/会话事务回滚。容器自己的 loopback HTTP 测试也执行通过。两项 ignored 仍是 setup 与 CLI 真实 PTY 测试：`setup_terminal_worker_releases_input_after_cancellation`、`idle_terminal_worker_joins_and_restores_terminal_on_shutdown`，原因明确且未计为通过。

Embedded 产物为 `/private/tmp/alia-platform-5aecb38.rTx8mP/cache/target/debug/alia-embedded`，已由 `file` 确认为 ARM aarch64 ELF 64-bit PIE，解释器 `/lib/ld-linux-aarch64.so.1`，BuildID `1e27ee9a122f1f4e048074a2a0fb32bb56d54494`。产物 SHA256 为 `1f4c0723437d8fdcc12bd4ce5099ffaeeb3b2882c91bba0174a2f4b89305378c`；本次重新编译了 `alia-proto`、`alia-transport`、`alia-core`、`alia-embedded` 并完成链接，未启动设备进程。

Sherpa 继续使用缓存中的官方 `sherpa-onnx-v1.13.1-linux-aarch64-static-lib.tar.bz2`，重新核对 SHA256 为 `86d0d3818770d4f6a087e52691d3ae171393972030f133df064a271fe9fe1749`。没有设置 `DOCS_RS`、关闭 Sherpa 或下载任何模型。Cargo.lock 相比 `ef07d43` 只增加了 `alia-embedded` 对 `alia-core` 的工作区依赖，原生归档内容仍需独立摘要约束。

### P2 复现与证据

从仓库根目录执行。下面使用相同已授权的专用测试库；应先确保该库可达，不能改指向用户或生产数据库。缓存复制仅用于加速，可省略后从空缓存构建。

```bash
validation_dir=$(mktemp -d /private/tmp/alia-platform-5aecb38.XXXXXX)
mkdir -p "$validation_dir/source"
git archive 5aecb38cc9bceb51f1b424c99f6e76f944f0a544 | tar -x -C "$validation_dir/source"
cp -cR /private/tmp/alia-platform-ef07d43.maWX2V/cache "$validation_dir/cache"
docker run --rm --platform linux/arm64 --cpus 6 --memory 12g --pids-limit 512 \
  --mount "type=bind,source=$validation_dir/source,target=/src,readonly" \
  --mount "type=bind,source=$validation_dir/cache,target=/cache" \
  --env CARGO_HOME=/cache/cargo --env CARGO_TARGET_DIR=/cache/target \
  --env CARGO_BUILD_JOBS=4 --env DEBIAN_FRONTEND=noninteractive \
  --env ALIA_TEST_DATABASE_URL=postgresql://alia_test:alia_test@host.docker.internal:32768/alia_refactor_p2_verify2 \
  --workdir /src \
  rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1 \
  bash -eu -c '
    apt-get update
    apt-get install --no-install-recommends -y ca-certificates pkg-config protobuf-compiler libprotobuf-dev libssl-dev libasound2-dev libdbus-1-dev cmake clang time postgresql-client
    rustc -vV
    cargo -V
    sha256sum Cargo.lock
    psql -X -v ON_ERROR_STOP=1 "$ALIA_TEST_DATABASE_URL" -c "SELECT current_database(), version();"
    cargo check -p alia-server --all-targets --locked
    cargo test -p alia-server --lib --locked
    cargo build -p alia-embedded --locked
    sha256sum Cargo.lock
  '
```

实际逐阶段脚本及原始日志保存在 `/private/tmp/alia-platform-5aecb38.rTx8mP/evidence/`，包括 `run.sh`、`image-inspect.json`、`environment.txt`、`system-packages.txt`、`database-preflight.log`、各阶段 `.command/.start/.end/.status/.log`、`lock-after.txt` 与 `embedded-file.txt`。每个构建阶段独立限时 2400 秒，使用 `/usr/bin/time -v` 记录资源和耗时；三阶段退出码均为 0。

本轮未执行 Server 集成测试目标、Embedded 单测或二进制实机运行，也未启动真实 Easy-Turn/LLM/TTS、下载模型或验证 Pi 5 音频、蓝牙、AEC 和双讲。Linux ARM64 结果不证明 Linux x86_64、Windows 或 macOS Intel 原生支持。该固定提交之后的生命周期和 Memory batch 修改必须另行验证，不能沿用本次结果。

## 历史验证：固定提交 ef07d43

以下记录只覆盖 `ef07d43b937682df1b600447f14c370cff6a5dfa`，不包含其后播放协议、凭证续期与 P2 修改。保留用于说明缓存来源和首轮环境修复。

## 范围与环境

- 从 `git archive` 导出已提交源码，容器以只读方式挂载。没有使用工作区尚未提交的代码。
- `Cargo.lock` SHA256：`b09b7803651b9586dd094e835e5c24e2623034f04a4a3b0e4f6ac5bc7c3efa2d`。Cargo 命令均使用 `--locked`。
- 官方镜像：`docker.io/library/rust:1.95-bookworm`，固定摘要 `sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1`；实际运行平台为 Linux ARM64 / Debian 12。
- 本次解析到的 ARM64 image ID：`sha256:7cf1e580ef5539f03b58560753e8ab84c8c360960d99dff714004aa98f203977`。
- `rustc 1.95.0 (59807616e 2026-04-14)`；host `aarch64-unknown-linux-gnu`；LLVM `22.1.2`；`cargo 1.95.0 (f2d3ce0bd 2026-03-21)`。
- Docker Engine 29.4.0，OrbStack Linux kernel `7.0.14-orbstack-00380-ga7e0a2dc9535`。容器限制 6 CPU、12 GiB 内存、512 PIDs，`CARGO_BUILD_JOBS=4`。
- 验证目录：`/private/tmp/alia-platform-ef07d43.maWX2V`。源码、Cargo registry、target、日志均独立；未挂载宿主 Cargo 缓存、Docker socket 或用户数据库。
- 容器名 `alia-platform-ef07d43-mawx2v`，退出自动移除。未操作已有 `alia-postgres` 或其他用户容器。

容器额外安装的系统构建依赖：

| 包 | 实际版本 |
| --- | --- |
| ca-certificates | 20250419~deb12u1 |
| pkg-config | 1.8.1-1 |
| protobuf-compiler | 3.21.12-3+deb12u1 |
| libprotobuf-dev | 3.21.12-3+deb12u1 |
| libssl-dev | 3.0.20-1~deb12u2 |
| libasound2-dev | 1.2.8-1+b1 |
| libdbus-1-dev | 1.14.10-1~deb12u1 |
| cmake | 3.25.1-1 |
| clang | 1:14.0-55.7~deb12u1 |

`protoc --version` 为 `libprotoc 3.21.12`。系统包版本反映本次实际安装结果；复跑时 Debian 仓库可能提供更新版本，应重新记录。

## 本次结果

以下为补齐系统依赖后、同一固定源码快照的实际结果。三条命令退出码均为 `0`，结束后的锁文件 SHA256 与开始前相同。

| 检查 | 命令 | 实际结果 | 本次墙钟耗时 |
| --- | --- | --- | --- |
| Server 默认 features，所有 targets | `cargo check -p alia-server --all-targets --locked` | 通过 | 13.58 s |
| Server 默认 features，纯库测试 | `cargo test -p alia-server --lib --locked` | 136 passed / 0 failed / 2 ignored | 31.05 s，测试执行 0.35 s |
| Embedded 默认 Sherpa features，完整构建 | `cargo build -p alia-embedded --locked` | 编译及最终链接通过 | 14.37 s |

默认 features 未被替换成 `--no-default-features`。Server 库测试在容器自己的 loopback 上运行；不连接真实 Easy-Turn、LLM/TTS、数据库或物理设备。

Server 测试包含真实容器 loopback HTTP 请求、Easy-Turn HTTP 契约/取消/容量测试，以及注入 detector 的持续采集与过期结果测试；模型结果是测试注入，不是真实模型推理。两项 ignored 分别是 `bootstrap::setup::tests::setup_terminal_worker_releases_input_after_cancellation` 与 `channels::cli::tests::idle_terminal_worker_joins_and_restores_terminal_on_shutdown`，均明确需要真实 PTY。本次未执行它们，也未运行 Server 集成测试套件。

Embedded 默认 features 为 `wakeword-sherpa`、`stt-sherpa`、`direct-response`。产物 `/private/tmp/alia-platform-ef07d43.maWX2V/cache/target/debug/alia-embedded` 经 `file` 确认为 ARM aarch64 ELF 64-bit PIE，解释器 `/lib/ld-linux-aarch64.so.1`，包含调试信息、未 strip；本次没有启动该设备进程。

Server check 无警告；Embedded 构建有一项非阻断警告：`packages/alia-embedded/src/module/discovery.rs:5` 的 `BTreeMap` import 在此 Linux 构建未使用。没有因此修改生产代码。

成功阶段时间为 2026-09-07 18:56:50–18:57:49 UTC（北京时间 2026-09-08 02:56:50–02:57:49）。复跑复用了下述首轮的专用 Cargo 缓存，因此这些数字不是全新缓存构建基准。已确认一次性容器退出并自动移除，专用日志及缓存保留。

## 首轮环境缺项与原生依赖

首轮使用 `apt-get install --no-install-recommends protobuf-compiler ...`，缺少 `libprotobuf-dev`。三阶段均在 `alia-proto` 的 build script 退出 `101`，共同报错：

```text
protoc failed: google/protobuf/empty.proto: File not found.
binding.proto:5:1: Import "google/protobuf/empty.proto" was not found or had errors.
binding.proto:15:24: "google.protobuf.Empty" is not defined.
```

补充容器内 `libprotobuf-dev` 后三阶段通过；没有修改 `crates/proto/build.rs` 或 `.proto`。这是复现环境必须列出的构建依赖，不能把首轮失败计作已证实的源码兼容错误。首轮原始日志保存在 `evidence/attempt-1-missing-protobuf-headers/`。

锁定的关键依赖包括 `sherpa-onnx 1.13.1`、`sherpa-onnx-sys 1.13.1`、`cpal 0.15.3`、`alsa 0.9.1`、`bluer 0.17.4`、`network-interface 1.1.4`、`tonic 0.13.1`、`openssl 0.10.79`、`openssl-sys 0.9.115`。

本次未设置 `DOCS_RS`、`SHERPA_ONNX_LIB_DIR` 或禁用 Sherpa。`sherpa-onnx-sys 1.13.1` build script 从官方 release 下载了 [Linux aarch64 静态库归档](https://github.com/k2-fsa/sherpa-onnx/releases/download/v1.13.1/sherpa-onnx-v1.13.1-linux-aarch64-static-lib.tar.bz2)，缓存于独立 `target/sherpa-onnx-prebuilt/`。实际归档 SHA256 为 `86d0d3818770d4f6a087e52691d3ae171393972030f133df064a271fe9fe1749`。

Cargo.lock 固定 Rust crate，并不固定该 build script 额外下载的原生归档内容；上面的摘要记录本次输入。发行构建需要保留并校验原生归档，可使用上游已有的 `SHERPA_ONNX_ARCHIVE_DIR` 指定经过校验的本地归档，不能仅凭 Cargo.lock 宣称原生输入也已固定。

## 复现

在仓库根目录执行；下面命令只创建独立临时目录和一次性容器。Cargo 缓存和编译产物保留在该临时目录，方便核验和后续手动清理。

```bash
validation_dir=$(mktemp -d /private/tmp/alia-platform-ef07d43.XXXXXX)
mkdir -p "$validation_dir/source" "$validation_dir/cache"
git archive ef07d43b937682df1b600447f14c370cff6a5dfa | tar -x -C "$validation_dir/source"
docker run --rm --platform linux/arm64 --cpus 6 --memory 12g --pids-limit 512 \
  --mount "type=bind,source=$validation_dir/source,target=/src,readonly" \
  --mount "type=bind,source=$validation_dir/cache,target=/cache" \
  --env CARGO_HOME=/cache/cargo --env CARGO_TARGET_DIR=/cache/target \
  --env CARGO_BUILD_JOBS=4 --env DEBIAN_FRONTEND=noninteractive --workdir /src \
  rust@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1 \
  bash -eu -c '
    apt-get update
    apt-get install --no-install-recommends -y ca-certificates pkg-config protobuf-compiler libprotobuf-dev libssl-dev libasound2-dev libdbus-1-dev cmake clang time
    rustc -vV
    cargo -V
    sha256sum Cargo.lock
    cargo check -p alia-server --all-targets --locked
    cargo test -p alia-server --lib --locked
    cargo build -p alia-embedded --locked
    sha256sum Cargo.lock
  '
```

本次逐阶段保留了 `/private/tmp/alia-platform-ef07d43.maWX2V/evidence/` 下的环境、镜像信息、系统包版本、命令、开始/结束时间、退出码和完整日志。每个 Cargo 阶段独立限时 2400 秒，并通过 `/usr/bin/time -v` 记录耗时及资源使用；失败后仍尝试下一独立阶段，避免把单一失败误判为所有目标均失败。

## 结论边界

Linux ARM64 容器结果不能证明 Raspberry Pi 5 实机、音频设备、蓝牙守护进程、Sherpa 模型质量、AEC 或自然双讲已通过。它也不证明 Linux x86_64、Windows、macOS Intel 原生支持。后续源码提交需要针对其变化重新验证。
