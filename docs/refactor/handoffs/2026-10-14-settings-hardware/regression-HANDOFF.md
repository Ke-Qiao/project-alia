# Settings 锁与取消 — 回归 HANDOFF

日期 2026-10-14；分工 `settings_auth_reproduce`。只新增 `packages/alia-server/tests/settings_cancellation.rs` 和本文；独立 integration test 自动发现，无 parent module、生产源码、依赖或配置修改。Cargo、数据库、Docker、Git 和本轮总证据由 Root 统一执行。

## Checklist

- [x] 读取 SettingsRegistry 的 update/persistence/inner 锁、GET/PATCH、模块 apply 和已有回归。
- [x] 检查当前 Axum 提取器及 HTTP runtime 的取消边界，不把客户端断开当作已证实的 handler 取消。
- [x] 新增两项精确排队／取消测试，并完成 scoped rustfmt；等待 Root 运行。
- [x] Root 执行验证并归档实际结果、完成本轮提交。

分工交回时为 **3/4**，当时测试尚未运行；Root 最终验证后为 **4/4**，实际结果见文末。软件测试不构成 R02/R03/R05 硬件证据。

## 新增两项离线测试

1. `dropping_a_queued_update_does_not_block_the_next_waiter`：先持有真实 registry update guard，将两份 `lock_updates` future 明确 poll 到 Pending。drop 第一份实际 Box-pinned future 后，第二份仍受原持有者限制；释放原 guard 后，第二份须在 1 秒内取得锁。没有 sleep，也不会误把 drop 一个 Pin 引用当成取消底层 future。
2. `aborting_an_update_holder_releases_admission_without_rolling_back_desired_values`：先保存受控数字旧值。task 取得真实 update guard 并完成 registry desired 更新后，用 oneshot 精确报告阶段，再停在 pending await。主测试确认后续等待者 Pending，明确 abort task 并等待 cancelled JoinError，随后确认等待者取得释放的 guard；desired 新值保留，磁盘旧文件不变，同值显式 save 后新 registry 能载入新值。失败时 catch 后 abort/join 未完成任务，临时目录按 scope 清理。

第二项只证明**真实 registry 任务取消在完成 desired 更新与开始 save 之间的行为**；它不模拟或声称测到 HTTP 断开、全部模块 apply 中止或跨域事务回滚。

建议 Root 的窄验证入口：`cargo test -p alia-server --test settings_cancellation --offline --locked`。该 integration target 不连接数据库、不监听端口、不调用模型或硬件。

## 复用的已有证据与部分成功边界

`tests/server_runtime/settings.rs` 已有真实 HTTP/PG 回归：

- `file_failure_keeps_live_settings_and_same_value_retry_persists_for_restart`：文件发布前失败保留 desired/live 与旧文件，同值显式请求仍 apply/save，再启动读取新文件。
- `module_partial_failure_saves_desired_settings_and_explicit_retry_applies_again`：模块先产生可见应用再返回失败，响应如实报告部分成功，desired 保存且显式同值重试继续应用。

`src/settings/storage/tests.rs` 已有排队 save 的精确 Prepared 屏障：后一个 save 等待 persistence 准入时不持 inner 读锁，准入后才取新快照；以及发布前／发布后持久性错误、重试同步完整父目录链等测试。本轮不再复制这些完整回归。

当前生产顺序：PATCH 的 policy CAS/最终身份准入后才更改 desired，模块逐个 await apply，随后更新 interaction policy 并保存。取消不会补偿已提交 SQL、已完成的 desired 或模块副作用；模块自行启动的工作也不能从调用 future 被取消推定已被撤销。原部分成功和显式重试政策不变。

`SettingsRegistry::save` 先 await persistence 锁，再 await inner 读锁并复制快照；后续 TOML 编码与 `storage::save` 均同步执行，没有协作取消点。因此不能声明 task abort 能在已经开始的同步文件发布中途立即终止写入，更不能据未收到 HTTP 响应判定未写入。当前原子存储及发布后持久性未确认语义保持不变。

## HTTP body／handler drop 与 task abort 的区别

已静态读取仓库 `auth/middleware.rs`、`api/settings.rs`、`bootstrap/runtime.rs`，并对照当前本地锁定依赖 Axum 0.8.9 的 `handler/mod.rs` 和 `json.rs`：handler service 顺序 await `FromRequestParts`，再 await 最后一个 `FromRequest`，随后才调用实际 handler。PATCH 的 Json extractor 会等待 body bytes 完整读取、解码；因此**慢 body 提取期间尚未进入 `update_settings`，也尚未持有其 update gate**，但先前 AuthUser 快照可能已形成，后续最终身份核验仍然必要。

当实际持有 guard 的 future 被 drop，RAII 会释放 guard；新增第二项以本地 task `abort` 后等待 `JoinError::is_cancelled` 证明 task future 已取消，不把发出 abort 请求本身当成清理完成。相反，丢弃 JoinHandle 本身不等于 abort task，取消 reqwest 客户端 future 也不证明服务器 handler 已 drop。当前 HTTP runtime 使用 `axum::serve(...).with_graceful_shutdown(...)`，没有为每个 Settings 请求建立“客户端一断开就撤销所有操作”的自有协议。

本轮没有执行 TCP/HTTP 断开、慢 body、私有 registry 内锁或同步文件写入中途的取消注入，不增加请求总超时、跨模块回滚、生产测试钩子或持续授权框架。R02/R03/R05 的设备、声学和长稳证据由对应分工独立推进。

## Root 最终收口

本轮审计与准备 checklist **8/8**。测试源码 `cffbca531cec2478c17b5bcacfd191e7f00a56f9`；冻结输入 **656/656** 与 Git blobs 一致，manifest `bb52f12217b548c4d152992101dd304448a0581fccb690c1967b97d9967b819c`。生产源码、API、政策、权限与依赖未改。

当前 macOS ARM64：Settings 原回归 **5/0/0**、新取消回归 **2/0/0**、存储回归 **7/0/0**、Embedded 默认 lib 软件基线 **177/0/0**。默认测试并行度，原期限保持，所有阶段输入无漂移；格式、架构边界正反自测和差异检查通过。原始命令、日志与哈希见 [证据索引](evidence-index.json)。

所有验证进程已退出；未创建/启动外部数据库、容器或常驻服务，未运行 SSH/录放音/串口/机构动作。Embedded 软件 fixture 可使用临时 SQLite 与回环 socket；stage JSON 的 database=not used 专指外部/PostgreSQL 数据库。

H01—H05 仍待当前部署确认，R02-A/R03-A 为下一可执行软件包，尚未实施；六轮仍 **2/6**，L09—L12 **4/4**。本轮不宣称当前 Linux/workspace/release、真实模型、AEC、控制板反馈或24小时整机验收通过。客户端断开/真实handler取消、慢模型与共享gate传导、同步文件阻塞仍待独立动态验证；R05前须关闭对应恢复门槛。
