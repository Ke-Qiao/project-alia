# Linux 软件发布验收 — Tests

2026-10-17；基线 `da3eca2`，当前已测软件提交 `4e2968f18d765f326e76d24a3bcdf5bba9034efc`。本分工负责只读核对 feature、测试矩阵、计数器、runner 和清理机制；Root 统一执行。本次审计 **5/5**、动态复核 **5/5**：本轮限定的 Linux 软件测试、编译、release 文件／加载器与资源回收证据通过；不代表普通 Server 进程部署或硬件验收通过。

## 审计 checklist

- [x] 核对当前 Cargo workspace/feature 和最近 Linux `d080710`、Mac `4e2968f` 证据边界。
- [x] 核对两基线间改动，确定完整 Linux default workspace 比分散定向更适合当前范围。
- [x] 核对 no-default all-targets、默认 release/native 链接及软件 null PCM 覆盖。
- [x] 给出可复用 runner、计数器、manifest/commit 校验及 exact ID 清理脚本路径，标出不能原样执行的旧硬编码。
- [x] 核对既有 Linux 播放 child hash 修复与原期限／并发约束，向 Root 交接；未运行 Cargo、Docker、数据库或硬件。

## 基线与本轮必须补上的覆盖

最近完成的 Linux ARM64 Server 默认完整证据是 `d080710052ae308b962b826d0f096fb10067469e`：Server **852 passed / 0 failed / 5 ignored**，Server no-default all-targets 通过；同轮 Mac workspace 为 1134/0/5。该轮没有运行 Linux 全 workspace、release 或部署 smoke。见 [原恢复检查点](../../resume-checkpoint-settings-read-authorization-2026-10-12.md)。

从 `d080710` 到当前软件 `4e2968f`，packages/core 范围有 85 个路径变化（含 package 内文档）；Cargo manifests、Cargo.lock 和 feature 选择未变。新增／改动覆盖 R03 停止保持、校准门槛、PCA AI/SLEEP 和可信动作内容，R02 双流证据、离线 replay、独立时钟及 Linux ALSA capture，静态 preflight，Settings 模型锁等待，V01 paired/fixture 初始化取消与排空，ServerStartup 取消，CompanionHistory。不能只重跑最后七项历史测试便宣称当前 Linux 软件已验收。

当前 Mac `4e2968f` 的一次 workspace 为 **1277/0/5/0 filtered**，其中 Server library 381/0/3、server_runtime 85/0/0、duplex_playback 31/0/0；677 输入逐 blob 对齐。该数量是 Mac 平台证据，不应硬编码为 Linux 预期总数。Linux 专用 ALSA 单元测试、PCM 名称测试等会造成平台差异；最终按本轮实际 Cargo target 计数及明确新增／缺失测试核对。

## 本轮实际验收矩阵

| 阶段 | 同一冻结源码上的命令／证据 | 范围与限制 |
|---|---|---|
| Linux ARM64 默认完整回归 | `cargo test --workspace --offline --locked` | 包含完整 Server、Embedded default、Core、Proto、Transport 和现有集成／文档目标；无需另跑同源 Server 全量或 Embedded 碎片求绿。可由 Root 在开始前选定既有 `--no-fail-fast` 收集全部目标，仍保留非零失败结果；不改变每项期限或默认并发。 |
| Server 无默认 feature 全目标编译 | `cargo check -p alia-server --no-default-features --all-targets --offline --locked` | 单独执行并记录 Server 可选 provider/channel 关闭后的编译分支及 examples/tests；它是编译检查，不是 no-default runtime 通过。 |
| Embedded 无默认 feature 全目标编译 | `cargo check -p alia-embedded --no-default-features --all-targets --offline --locked` | 单独执行并记录 Embedded Sherpa 关闭后的编译分支及 examples/tests，避免联合 feature 解析掩盖各包实际范围；不宣称 no-default runtime 通过。 |
| 两个默认 release 可执行文件 | `cargo build -p alia-server -p alia-embedded --release --offline --locked` | 正常实际链接 native 库；禁止 `DOCS_RS` 跳过 Sherpa 链接。记录每个可执行文件 bytes、SHA-256、`file` 的 AArch64 ELF 身份和 `ldd`，不能有 missing library。 |
| release 的文件与安全装载检查 | 两端 ELF、loader `--verify`／`--list`、`ldd`，仅 Embedded 执行 `--help` | 本轮不运行 Server 普通进程，不执行 Embedded `--check-config`。Embedded help 在 runtime/model/device 初始化前返回。**Server 没有 `--help` 参数分支**，该参数仍会进入正常 setup/startup，不能当无副作用 smoke；本轮文件／加载器结果不代表部署启动验收。 |
| 同源交付记录 | manifest/commit、阶段 drift 和资源记录 | 本轮没有源码改动，沿用已完成的同源 Mac 格式／边界检查，不重复执行；也不重复 Mac workspace。若发现并修复软件缺口，再按实际变更决定必要回归。 |

当前 Embedded default 仅 `sherpa-onnx`，`vision-opencv` 是可选未选 feature；本轮不启用 `--all-features`、视觉部署、新模型或新原生依赖。Server default 包含现有 embedded channel、HTTP provider/tools、insights、home-assistant 和 turn-easy；普通测试使用受控本地服务，不能由编译或 mock 通过宣称外部模型验收。

Linux `alsa`、`bluer` 和 CPAL 并不因 `--no-default-features` 消失，仍需既有 ALSA/DBus native 构建环境。默认 workspace 会执行 `module/cpal_audio/alsa_backend.rs` 的七项真实 libasound **软件 `null` PCM** 单元测试，以及 `tests/audio.rs` 的 Linux 名称解析分支。它们验证 native 调用、reference/capture/恢复/close 行为；不挂载 `/dev/snd`、I²C/USB/GPIO 或启动宿主蓝牙服务，不证明声卡、R1 AEC、Pi 或物理录放音。PCA 寄存器测试仍是受控 writer，并非真实执行器动作。

五项既有 ignored（3 PTY、2 Easy-Turn）须在本轮单独列出。不能新增 ignore、过滤刚加入的用例、降低并发、改 CASE permits 或只以失败测试复跑覆盖首次失败。若完整运行失败，先保留本轮原件和资源现场，定位具体缺口；新版本验证与原失败分开计数。

## 可复用的脚本与精确路径

以下均是已存在脚本，**含旧任务名称、DB 和目录常量，不可原样执行旧任务**；Root 可复制到本轮 ignored run 后只调整本轮身份和输出路径。所有命令及各自日志单独保留，不修改历史证据。

- Linux 阶段 runner：`run/validation-20261012-settings-read-authorization/linux-arm64-v3/evidence/run-stage.py`，版本化归档为 `docs/refactor/handoffs/2026-10-12-settings-read-authorization/evidence/linux-arm64-v3/evidence/run-stage.py`。它在命令前后比较只读 source manifest，记录命令／退出码／时间，拒绝覆盖同名阶段。保留原进程 watchdog 1800 秒和 terminate 后 10 秒 kill 兜底，`RUST_TEST_THREADS`/`RUST_MIN_STACK`/`DOCS_RS` unset；`CARGO_BUILD_JOBS=2` 只是编译并行，不修改 libtest 并发。L12 同类 runner 会在完整测试时采样 PG，若需要阶段故障定位可复用而非新增生产观测。
- 计数器：`run/validation-20261012-settings-read-authorization/count_cargo_tests.py`，以及同目录 `test-counter.py`。八例自测覆盖 Docker stdout/stderr 前后乱序、嵌套 child 摘要和歧义拒绝。示例接口：`python3 count_cargo_tests.py <完整日志> --result <阶段result.json> --json <计数输出> --text <表格输出>`。sidecar 需有 `finished_at`、exit_code、source_drift；只计每个 Running/Doc-tests 边界内最后一个外层 summary，并要求其数量匹配外层 `running N tests`。独立 Cargo 命令不能混进同一日志，RED／定向／不同 feature 的同名测试不能累计成更多通过。
- **不要直接复用上一轮 Mac 的 `run/validation-20261016-invitation-history/summarize.py` 断言**：其中写死 `1270 + 新7项`、72 targets 和 Mac 特定结果。Linux 应使用上面的通用计数器，并对实际目标与 cfg 差异解释，不随意删掉失败断言凑总数。
- 冻结／提交逐 blob 对齐：`run/validation-20261016-invitation-history/freeze-source.py`、`verify-commit.py`；归档同目录 `archive.py` 可保留字节数/SHA-256 索引。源清单包含非忽略 untracked 构建输入，排除顶层 docs/run/私有agent元数据；当前参考为 677 项，最终以本轮清单为准。
- 资源观测与清理：最新 `run/validation-20261016-invitation-history/resources.py`、`cleanup.py` 有容器 exact ID + task label 核验，并在销毁前采集 schema/client。失败也要先保存实际残留，再做任务范围兜底；不能沿用旧 `collect-resources.py` 中自动 DROP 遗留 schema 后才写“干净”的处理来掩盖清理缺口。仅停止／删除本轮明确拥有的 holder/PG/卷，不操作用户 DB、共享运行环境和缓存。
- release/link 原命令模板：`docs/refactor/handoffs/2026-10-06/evidence/linux/release.command.json`、`binaries.command.json`；旧 no-default 联合命令在同目录 `no-default.command.json`，本轮改为上表两个包分别执行。旧 `release-smoke.py` 不在本轮执行范围，本轮不进行普通进程／配置演练。

历史 Linux 环境记录是 rustc/cargo 1.95.0、aarch64-unknown-linux-gnu、Debian 12，不能当 Ubuntu 24.04 Pi 运行验证。既有 Sherpa Linux ARM64 静态归档是 `sherpa-onnx-v1.13.1-linux-aarch64-static-lib.tar.bz2`、19,167,687 bytes、SHA-256 `86d0d3818770d4f6a087e52691d3ae171393972030f133df064a271fe9fe1749`；本轮只可重新核验既有实际归档/镜像/库，不把旧记录当作本轮已经链接的证明。

## 原期限与已知 hash 问题

当前根 Cargo.toml 保留 L12 已验证的 `[profile.dev.package.sha2] opt-level = 3`。当时同一 462,699,808 bytes Linux debug ELF 的真实同算法 SHA 从 15.416 秒降到 0.834 秒，child 从 16.551 秒降到 1.280 秒；没有更换算法或缩减身份输入。见 [L12 playback HANDOFF](../2026-10-10-l12/playback-HANDOFF.md)。这项已修复的 hash 启动成本不应重新认定为未修 native cleanup 卡死，也不能反推历史 V01 的全部超时原因。

必须保留：playback panic child 原 20 秒父期限；Server runtime fixture `DATABASE_CASES=2`、正文45秒；delivery support `CASES=3`、原45秒 body+context cleanup deadline，超期仍保留同一 cleanup owner 排空后再清 DB；Pipeline `PIPELINE_DATABASE_CASES=2` 和15秒行为期限。Pipeline 既有真实执行文件身份初始化已放在 fixture setup 中，不能另外移动生产关键工作到期限外或提前伪造缓存。

本轮若该 child 再失败，先读取原 `[runtime-phase]` 的 build/native/retirement/schema/exit，以及同库等待证据；只在观测显示 hash 耗时值得重新验证时再用 L12 `measure-build-digest.py` 和 `build-digest-probe.rs` 对同一实际 binary/fingerprint 做测量，不预先追加重复诊断运行。

## 实际 workspace 结果与独立复核

本轮 `run/validation-20261017-linux-release/evidence/workspace-default.{command.json,result.json,log}` 记录一次默认 `cargo test --workspace --offline --locked`，exit **0**，总计 **483.737 秒**。677 项源在命令前后一致，`source_drift=[]`，清单 SHA-256 为 `c239c76799e4f70096c2c7e5898e396534beb2a762fb7628203c671d2941c5bf`。environment 原件确认 ARM64 Debian 12、rustc/cargo 1.95.0；同源测试没有使用 `--test-threads`、`--ignored` 或过滤参数，`RUST_TEST_THREADS`/`RUST_MIN_STACK`/`DOCS_RS` 均 unset。

已分别读取原日志、`workspace-counts.json` 和 `.txt`，独立按每个 Cargo Running/Doc-tests 边界的最后摘要重算，并核对日志 SHA-256 `20a7b25d3f468756b81d909213bc77dfd851c02c3bbc176da504b389bc8ec211`：**67 Running + 5 Doc-tests，1285 passed / 0 failed / 5 ignored / 0 filtered**。五个 Doc-tests 目标均为零测试；三个嵌套子进程摘要的 3 passed／1149 filtered 未加入外层数量，另外一个故意退出子进程没有独立摘要。计数器 `parse_issues`、orphan summaries 和 Cargo errors 均为空。

逐目标对照同源 Mac 的 72 个目标，只有下列两项计数变化；其余 70 个目标数量和已打印测试状态相同，没有 Mac 测试消失：

| 目标 | Mac | Linux | 原因 |
|---|---:|---:|---|
| Embedded library | 255/0/0 | 262/0/0 | `cfg(target_os = "linux")` 启用 `alsa_backend` 的七项测试，原日志七项均 `ok`。 |
| Embedded `tests/audio.rs` | 9/0/0 | 10/0/0 | Linux 专用 `detects_alsa_pcm_names` 为 `ok`。 |

其余重点目标为 Server library **381/0/3**、`server_runtime` **85/0/0**（76.73 秒）、`duplex_playback` **31/0/0**（45.45 秒）。目标总时长包含多个并发／限槽用例，不是放宽单用例期限的证据；原 fixture 期限、`DATABASE_CASES=2`、delivery `CASES=3`、Pipeline `PIPELINE_DATABASE_CASES=2` 均保留。

五项 ignored 与 Mac 原记录逐名一致，本轮没有新增 ignored：

- `bootstrap::setup::tests::setup_terminal_worker_releases_input_after_cancellation`：真实 PTY。
- `channels::cli::tests::idle_terminal_worker_joins_and_restores_terminal_on_shutdown`：真实 PTY。
- `channels::cli::text::tests::real_console_write_proves_utf8_bytes_and_restores_terminal`：真实 PTY。
- `official_audio_through_production_rust_adapter`：真实 Easy-Turn 服务与官方 PCM。
- `official_audio_over_grpc_preserves_original_response_across_backchannel_and_wait`：真实 Easy-Turn 服务、PCM 目录及隔离数据库。

数据库执行不是仅凭“未见 skip 文本”推定：本轮 `run-stage.py` 在实际 `docker exec env` 参数中设置隔离 `ALIA_TEST_DATABASE_URL`，environment 原件也读取到该值；Server runtime、delivery、Companion/邀请和 Pipeline fixture 均对变量缺失或连接失败硬失败。现有 `plugin_storage` 有缺变量时直接返回的旧分支，本轮变量已设置，因此不进入该分支，其 `postgres_plugin_storage_round_trips_records_and_policy` 已通过。成功测试的 stdout/stderr 默认会被 libtest 捕获，不能将日志中无 skip 文字单独当成不跳过证明。

原播放 panic child 的真实阶段证据显示 `server.build` **1016 ms**，native 原 owner 放行／返回／join、schema 删除和 fixture cleanup 均完成，`child.exit` 为 **1376 ms**，位于原20秒父期限内；没有为本轮调整 hash 或超时，也未追加重复诊断。`after-workspace-resources.json` 是同一任务数据库在 workspace 结束 **11.165 秒**后的原始只读观测：**0 schema / 0 other client**。该观测没有先 DROP 遗留资源，也不冒充退出瞬间或 release 后最终资源状态。

## 动态 checklist

- [x] 核对 environment exit 0、当前工具链和冻结的 677 项源；workspace 前后无漂移。
- [x] 独立复核完整 workspace 原命令、退出码、计数、五项既有 ignored、八项 Linux cfg 差异及数据库非跳过条件。
- [x] 核对 workspace 后同库资源原始观测为 0/0，并保留实际 11.165 秒采样间隔。
- [x] 核对分别执行的 Server／Embedded no-default 及两个 default release、ELF／loader／Embedded help 原件。
- [x] 核对所有阶段完成后的资源、exact ID 清理和 artifact/manifest/既有源码 commit 对齐；不将未执行的普通 Server 进程退出或现场设备清理列为本轮已验收。

## 编译、release 与最终资源收口

已独立读取本轮四个阶段的 command/result/log，均 exit **0**、677 项输入前后一致、`source_drift=[]`，没有 runner error：

| 阶段 | 实际结果 | 证据文件前缀 |
|---|---|---|
| Server `--no-default-features --all-targets` | 21.055 秒 | `server-no-default` |
| Embedded `--no-default-features --all-targets` | 6.163 秒 | `embedded-no-default` |
| Server + Embedded default release | 115.452 秒 | `release-default` |
| ELF／加载器／库与 Embedded help | 0.405 秒 | `release-link-audit`，详细 `release-audit.json` |

`release-audit.json` 的 15 条记录命令均 exit 0：两端均为 ARM AArch64 ELF，解释器为 `/lib/ld-linux-aarch64.so.1`，`--verify`／`--list` 及 `ldd` 通过，没有缺失库或 RPATH/RUNPATH。Embedded 定义 170 个 `SherpaOnnx*` 符号，证明本次实际默认 native 链接；`--help` 成功且 runtime 目录仍空。**没有执行 Server 普通启动，也没有执行 Embedded `--check-config`、模型推理或设备操作。** `native-inputs-after.json` 确认既有 Sherpa 归档摘要相同、15 个静态库复核无差异。

独立读取 retained artifact 副本并重新计算 bytes/SHA-256，与 `retained-artifacts.json` 和 release 审核文件一致：

| 独立副本（`run/validation-20261017-linux-release/artifacts/`） | bytes | SHA-256 |
|---|---:|---|
| `alia-server` | 46,092,832 | `3528c5e02708e126d560cdaade8a431c0ed3e60de6c86892f931dcb0250de330` |
| `alia-embedded` | 49,540,200 | `159becdbd3783b77ad308d02096ed39158fc54cfef43f648091a2c0f015f4003` |

`baseline-commit-verification.json` 逐 blob 将 677 项清单与基线 `da3eca2fe6853f104dd558051aebf92dd62f067b` 对齐，mismatches/extra/missing 均为空；该基线的软件内容来自 `4e2968f`，本轮没有源修改。最终文档提交身份和版本化证据索引由 Root 统一记录，不声称产生了新软件修复提交。

已读取完成后的 `cleanup.json`，`complete=true`。回收前 Linux holder 的 `docker top` 仅余 `sleep infinity`，同一隔离数据库仍为 **0 schema / 0 other client**；随后逐一验证两个容器 exact ID、任务 label 和 internal network ID，再停止／删除本轮 holder 与 PG 容器、匿名 PG 卷及 internal network，后置查询确认均已移除。没有用清理之后的零值替代回收前观测，没有停止其余用户容器；共享 registry/target cache 和独立 release 副本保留。

本分工未运行 Cargo、Docker、测试或清理命令，未修改源码；动态结论来自对 Root 本轮原件的独立只读复核。R03 保持暂停，R02 实机声学、真实模型和 R05 整机验收边界不变。
