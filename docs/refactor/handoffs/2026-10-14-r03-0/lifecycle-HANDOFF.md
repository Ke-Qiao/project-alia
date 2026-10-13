# 2026-10-14 R03-0 — 停止保持与校准启动 HANDOFF

分工 `settings_auth_fix`，基线 `125ea58`。本分工实现生命周期与配置迁移；PCA 寄存器修复由另一 subagent 独占 `servo.rs`。Root 统一运行 Cargo、保存证据和提交，本分工不提交 Git、不运行独立 Cargo，不做 SSH 或硬件 I/O。

## 当前状态与 checklist

**实现、独立静态审查与 Root 统一 GREEN 均完成。** 默认 features 的 Embedded lib 和全部 integration targets 共 263 项通过、0 失败。源码已提交为 `b5100d7`；不能将此状态写作硬件保持通过、已校准或已部署。

- [x] 检索构造入口、启动/停止、plan 调用、配置与测试 fixture。
- [x] 在旧生产实现上增加两项 mock RED；Root 实际运行均按预期失败，期间无源码漂移。
- [x] 删除停止后自动 reset，保留精确取消、native owner、stop waiter drop 后继续 join。
- [x] 添加默认 false 的 `calibration_confirmed`，未确认的 native 构造/start 拒绝。
- [x] 未确认的 mock 无矩阵加载、无 writer 启动、无输出、不声明 `motion.native`。
- [x] 校准确认仍必须通过原矩阵解析、输出维度和中心角限制验证。
- [x] 启动只提交一次已校准中心目标，完整提交前不开放命令准入，不虚构初始位置。
- [x] 删除旧 reset/soft-start 四个配置键；TOML、loader、JSON 更新拒绝，保存不再输出旧键。
- [x] 补 complete/partial/cancel 启动、Close/binding 保持与 observer drop 所有权用例，并迁移 gRPC fixture。
- [x] rustfmt 完成；独立 reviewer 读回 7 个变更文件后报告无阻断。
- [x] Root 运行默认 features Embedded lib 与所有 integration targets，读回日志并回填真实 GREEN 结果。
- [x] Root 完成源码提交 `b5100d7`，最终证据归档、总 checklist/检查点随本交接记录提交。

本分工清单 **12/12**，最终证据已归档并随本交接记录提交。实机验证 **0 项执行**。

## RED 证据

Root 在旧生产源码上冻结 657 个输入，保存 `run/validation-20261014-r03-0/red-source.patch` 与 `red-source-manifest.json`。两项生命周期命令均使用默认 features 的 `cargo test -p alia-embedded --lib <filter> --offline --locked -- --exact --nocapture`；默认测试并行、构建 jobs=2、无数据库。

| RED filter | 实际失败 | 日志与元数据 |
| --- | --- | --- |
| `module::motion::runtime::tests::shutdown_waiter_drop_keeps_join_and_holds_after_entered_write_returns` | 旧实现 3 帧，要求 2 帧；多出的停止 reset 违反保持。退出 101，0.34 秒 | `run/validation-20261014-r03-0/red-stop.log`、`red-stop.json` |
| `module::motion::runtime::tests::unconfirmed_calibration_startup_does_not_submit_native_targets` | 旧实现发送 1 个中心帧，要求 0；未确认校准仍发生 startup 写入。退出 101，0.14 秒 | `run/validation-20261014-r03-0/red-startup.log`、`red-startup.json` |

两份元数据的 `source_drift=[]`；共同 manifest SHA-256 为 `3776a602beb56de64be8c33bd2da3d4fae642b9ae604efe3fd02ec7c761225e3`。RED 先释放 native 屏障并完成 stop，再断言，不用遗留阻塞任务制造失败。Root 持有最终证据归档责任。

## 最终行为

`packages/alia-embedded/src/config/types.rs` 的 `MotionConfig` 增加 `calibration_confirmed=false`，`hardware_enabled` 仍默认 false。该布尔是**本地操作者对实际机构校准的声明**，不是软件测量或仅文件存在的证明；目前用户没有校准资料，不应设为 true 来跳过前置条件。

`MotionRuntime::new_with_boot` 先验证 `hardware_enabled => calibration_confirmed`，后者未满足时在校准文件加载与后端构造前返回错误。`start` 再次验证同一条件，避免内部 fixture/库对象越过启动边界。确认后继续原 `NeckMapper::load`、servo count 与中心角范围检查；不能仅靠开关接受无效矩阵。

未确认且硬件关闭的默认 mock 可正常构造/start，但保持不可用，没有加载示例矩阵、spawn writer 或调用后端，也不会声明 `motion.native`。这样默认嵌入端的其他模块仍可工作，示例资产不会被误认作实际机构的校准。

`writer.rs` 的 `previous` 从 `None` 开始。校准后的 startup 只发送一个经 mapper 验证的中心目标；完整提交成功后，才创建 last submitted software target 并开放 motion。初始实姿态仍然未知，不能从虚构的中心位置插值以宣称首段运动受限速保护；后续命令/face 继续对已提交的软件目标限速。全程不把中心提交解释为已到位，`position_confirmed_actions` 保持 None。

启动仍使用既有 owned blocking writer。部分/不确定 native 写入封闭 writer、保持失败事实，不重放中心；stop 在启动 syscall 已进入时仍等待原 owner，不追加恢复姿势。`ActiveWork::Reset` 改名为 `Startup`，避免生命周期中还存在可误用的 reset 分支。

停止后的 reset 代码全部删除：stop/drop、准确 lease Close、binding revoke、授权到期均不会新增归中或 disable 帧。已经进入的 native write 仍可能完成，软件必须保留其真实提交证据。单一 observer/RPC waiter 被丢弃仍不取得撤销 owner 或整条 lease 的权限；网络不可达仍沿用授权到期机制，不承诺断网瞬间的物理停止。

## 配置迁移范围

`MotionConfig` 使用 `deny_unknown_fields`，以下四个旧键不再被接受；包括显式 false，也不能静默忽略：

- `reset_on_stop`
- `reset_duration_ms`
- `soft_start_enabled`
- `soft_start_duration_ms`

两个 soft-start 参数原来从假定的中心初值插值，不能证明从实际未知姿态平滑启动，因此与旧 reset 参数一起移除。示例配置增加校准确认说明，默认 false；没有自动选择 PCA、开启硬件或写入任何校准值。

TOML 解析、`load_config`、`EmbeddedSettingsManager::update_from_json` 共用该结构验证。新增用例要求旧 JSON 键失败后内存快照和文件逐字保持，保存/重载的新配置不含旧键。gRPC mock fixture 显式确认它自己的模拟校准；这不构成实机授权。

## 变更文件与验证覆盖

本分工只改以下 7 个代码/配置文件，`plan.rs` 和 `servo.rs` 未修改：

- `packages/alia-embedded/src/module/motion/writer.rs`：未知初始 target、单次 startup 中心、无 stop reset。
- `packages/alia-embedded/src/module/motion/runtime.rs`：校准准入、未确认 mock、能力声明与 Startup 状态名称。
- `packages/alia-embedded/src/module/motion/runtime/tests.rs`：RED 迁移与新增生命周期边界。
- `packages/alia-embedded/src/config/types.rs`：新确认字段、拒绝旧字段、删除旧默认函数。
- `packages/alia-embedded/src/grpc/body_service/tests.rs`：确认模拟校准的 fixture。
- `packages/alia-embedded/tests/config.rs`：默认值、TOML/load/save 与 JSON 失败无变更。
- `config/embedded.example.toml`：未校准默认与未知初姿态说明。

新增/改写核心测试包括：stop waiter 被丢弃后 join 且无 reset；未校准 mock 零写；未校准 native 构造/start 拒绝；不加载矩阵或广告 capability；中心完整提交前不准入；startup 部分失败不重试；确认后仍检查矩阵和中心范围；Close/binding revoke 等待已进入 write 后保持；observer drop 不取消 owner。既有启动 waiter 取消、expired authorization、精确取消、generation/voice fence、部分通道、全局帧时钟等用例保留。

## GREEN 结果与剩余边界

Root 实际运行 `cargo test -p alia-embedded --all-targets --offline --locked`，退出 0，总耗时 89.4 秒。`run/validation-20261014-r03-0/green-embedded.log` 与 `green-embedded.json` 已由本分工读回：lib **199 通过 / 0 失败 / 0 忽略**，integration **64 通过 / 0 失败 / 0 忽略**（其中 config 15 项），共 **263**；上述新增生命周期与旧键配置测试均在日志中明确通过。binary target 为 0 项，不另计入通过数。

默认测试并行、构建 jobs=2、无外部数据库；Embedded 集成 fixture 使用临时 SQLite。657 个输入的 GREEN manifest SHA-256 为 `6c79ccc09484f47fd69ca8e5e93a243cb08a4b53dacfe36c3c02f02d349875ff`，`source_drift=[]`。Embedded 格式检查通过；Root 报告全局格式检查受未修改的基线 Server import 顺序阻断，因此不宣称 workspace fmt 全绿。

本轮 Linux 未运行：Root 记录的 Linux daemon 为 stopped，环境证据见 `run/validation-20261014-r03-0/environment.json`。未启用硬件、未连接 Pi；校准文件安全性、实际 PWM、供电、OE、负载、断连后的机械保持和首个中心动作均未实机验证。软件 GREEN 不替代这些硬件证据。

Root 收口：源码提交 `b5100d7`，657 个测试输入与 Git blob 全匹配；最终归档和恢复记录见 [Root HANDOFF](root-HANDOFF.md)。本清单12项均已关闭，不改变实机未验证状态。
