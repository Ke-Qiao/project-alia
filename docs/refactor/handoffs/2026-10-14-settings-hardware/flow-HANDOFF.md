# 2026-10-14 Settings 锁等待与取消 — 有界审计 HANDOFF

分工 `settings_auth_fix`。本轮仅静态审计 Settings GET/PATCH 的锁、预算、副作用与取消所有权，未修改生产源、测试、API、政策、权限、依赖或平台；未运行 Cargo、数据库、Docker 或 Git。以 [最新恢复检查点](../../resume-checkpoint-settings-read-authorization-2026-10-12.md) 为入口，其源码提交为 `d080710052ae308b962b826d0f096fb10067469e`，已完成上一轮 GET/PATCH 最后身份准入。本文的路径行号来自本轮读取的源码。

## 结论与检查项

此项有界静态审计 **8/8 完成**，可以交接并进入 R02/R03。没有发现需立即回退权限、政策或文件原子性的新缺陷；确认存在尚未给出统一时限的排队和 apply 路径，以及缺少动态证据的取消边界。它们属于可用性与恢复验收项，不能由上一轮 2 秒 SQL/身份检查或全量测试通过替代。它们不自动启动全面 Provider/R04 重构，也不构成 R02/R03 准备工作的阻断。

- [x] 读取最新检查点及上轮边界，限定本轮范围。
- [x] 逐项核对 Settings GET/PATCH 的全部 await 和 gate 生命周期。
- [x] 核对 Registry 的 update/inner/persistence 锁顺序及文件阶段。
- [x] 核对 SQL current/CAS、最后身份核验的既有预算与提交边界。
- [x] 核对内建 apply 路径及真实远端请求的现有超时，未把“无整体预算”一概称为远端永久无界。
- [x] 区分 future 被丢弃、客户端断开、服务器关闭、同步文件调用等取消条件。
- [x] 检索现有测试，提出最少的局部故障验证，不直接套整请求 timeout。
- [x] 报告可进入 R02/R03 的结论及 R05 前仍须关闭的验收门槛。

以上 **8/8 是静态审计完成数**，不是故障注入通过数、R05 完成数或项目完成百分比。

## 阶段、锁与副作用

| 阶段与源码位置 | 持有/等待的锁与现有预算 | 取消与状态边界 |
| --- | --- | --- |
| 入口认证 `auth/middleware.rs:35` | 当前账号 DB 查询 2 秒；尚未取 Settings gate | 原 JWT/active 检查与错误保持；不把等待中的 JWT 当持续授权租约 |
| GET `api/settings.rs:46`、PATCH `:76` | `SettingsRegistry::lock_updates` (`settings/registry.rs:79`) 获取同实例 Mutex，无排队期限 | 真正 drop 此等待 future 不会取得 gate/启动请求设置修改；不等于客户端 socket 一断就已 drop handler |
| current `api/settings.rs:317` | 2 秒包含事务 begin、policy `FOR UPDATE`、DB clock、可能写回和 commit；外层已持 update gate | `modules/memory/context/policy.rs:219` 可独立恢复到期 pause；取消不能一概声称 SQL 零变化 |
| GET 政策投影 `api/settings.rs:50` | 等 Registry inner 写锁；取得后同步批量更新，无内部 await | 是原 SQL 政策 RAM 投影；之后拒绝也不撤销它 |
| GET entries `api/settings.rs:52` | list/all 等 inner 读锁，取得后同步克隆；无阶段 deadline | 原 secret/schema/prefix 保持。entries 后 `:57` 最后身份查询 2 秒；成功后 JSON/ETag 同步构造，没有 handler await |
| PATCH 两次 preview `api/settings.rs:78`、`:84` | 没有阶段 deadline，仍持 update gate；trait 声明纯预览 | OpenAI embedding `providers/embedding/openai.rs:301` 仅读锁 clone 后计算，不发布配置；在这里 drop 不应用请求设置（先前 current 到期转换除外） |
| PATCH 政策/embedding CAS `api/settings.rs:108`、`:295` | 2 秒；SQL 内 policy `FOR UPDATE`、actor active admin `FOR SHARE`、版本/descriptor/pins 原子提交 | 事务 commit 确认超时可能是结果不确定，不可称整体回滚；原固定提示要求 reload |
| PATCH 非政策最后准入 `api/settings.rs:123` | `require_current_admin` (`:241`) 2 秒 id-only 查询，无跨 apply SQL 行锁 | 失败前不改请求 desired/live/file；通过后不是持续撤权租约 |
| PATCH desired `api/settings.rs:147` | inner 写锁，取得后同步完成整次 batch；`get_entry` (`:166`) 还会逐项等读锁 | 从这里起取消可留下 desired 已变；政策 CAS 可能更早已提交 |
| PATCH modules `api/settings.rs:187` | `bootstrap/registry.rs:129` 顺序 await 内建模块与 extensions，无统一 deadline；全程保留 update gate | 每个模块可以已部分生效再出错/挂起；原返回错误后继续后续模块。drop 只停止以后尚未执行的步骤，不撤销先前效果 |
| PATCH interaction `api/settings.rs:195` | 等 InteractionPolicies 写锁，取得后同步应用 | 此时已有 SQL/desired/模块变化；不能称取消无副作用 |
| PATCH save `api/settings.rs:212` | persistence Mutex → inner 读锁取快照（`settings/registry.rs:247`）；无排队 deadline | 等待阶段可 drop 当前 future，释放已持有 guard；但此前 desired/live/SQL 保留 |
| 同步文件发布 `settings/registry.rs:268`、`settings/storage.rs:45` | 序列化、创建/写临时文件、fsync、rename、逐祖先 fsync；没有 await | Tokio timeout/abort 无法在一次同步 poll 内抢占文件调用；返回后才可交还执行器。无独立后台保存 task，但不承诺阻塞文件系统可取消 |

Settings update gate 保留到 handler 返回或其 future 真正 drop。第三个共享调用者是 `api/memories.rs:526` 的管理员 privacy PATCH；它也会被慢 Settings apply 挡在 gate 外，因此应在 R05 管理可恢复性用例中考虑，不能只测读取页面。

Registry 内部锁顺序：`save`、`load_persisted`、`set_persist_path` 均先 persistence 再 inner；普通 get/list/all/batch/set 只取 inner，持有 inner 后没有 await 或反向获取 persistence。`save` 释放 inner 读锁后才同步写文件。审计未发现这些当前方法之间的确定锁顺序环；缺少期限不等于已证明 deadlock。

## 内建模块的实际等待与预算

`ModuleRegistry::apply_settings` (`bootstrap/registry.rs:129–182`) 先按实例去重 LLM，再顺序调用 STT、TTS、embedding、cache、skills、router、insights、rule_engine、tools，最后 extensions。不能把这些 borrowed futures 误说成已 detach 的任务；wrapper 本身没有 spawn。但 `ModuleLifecycle::apply_settings` (`modules/platform/lifecycle.rs:43`) 仅约定返回已消费键，没有通用取消安全或最大耗时契约，不能对任意 extension 承诺回滚。

大多数内建 apply 先获取本地锁，再在无 await 的代码中改配置：OpenAI LLM `providers/llm/openai.rs:452`、Anthropic `:270`、Gemini `:316`、Whisper `providers/stt/whisper.rs:187`、Embedding `providers/embedding/openai.rs:198`、Router `modules/interaction/routing/service.rs:473`、Insights `modules/platform/insights/service.rs:1178`。其局部赋值本身无网络 await，但锁持有者不一定同样短。

确认的实际传导路径为：

1. OpenAI `chat_internal` 在 `providers/llm/openai.rs:285` 持 config 读 guard 跨 `.send().await` (`:319`) 和错误 body (`:324`)；Responses 分支同样在 `:337` 持有到网络返回。Anthropic `providers/llm/anthropic.rs:141`、`:192`，Gemini `providers/llm/gemini.rs:176`、`:219` 也有相同模式；两者非流式路径还持 guard 跨成功 JSON body。
2. 上述 LLM apply 等对应 config 写锁，而 Settings 仍持全局 update gate。一个在途慢模型响应即可延迟 PATCH 及后续 Settings GET/PATCH/隐私修改，不要求有异常 extension 才能产生。
3. 这些 LLM 构造使用 `Client::new()`（OpenAI `:265`、Anthropic `:122`、Gemini `:158`），聊天请求没有显式 reqwest timeout。已核对锁定 `Cargo.lock:3122` 的 reqwest 0.12.28 及本地依赖源码：`src/async_impl/client.rs:299/313/314` 的 connect/read/total 均为 None，`:1439–1465` 文档明确默认无这些超时。默认 tcp_user_timeout 30 秒不能视为 HTTP response deadline。
4. 上层有局部保护，例如 routing `service.rs:326`、`:381` 以 `config.timeout_sec` 包住分类调用；VoiceTurn `modules/interaction/turn.rs:231` 的 `cancellable_stage` 是取消 token select，未给出统一时间 deadline。不能据此声称“所有模型调用永久无界”，也没有已验证的有限上界覆盖全部模型调用。实际慢请求故障传导尚未动态复现。

TTS apply 是明确的部分成功例外：`providers/tts/gpt_sovits.rs:830–962` 先改变并释放本地 config，`:971` 再按变更键执行远端模型权重配置。`configure_remote_weights` (`:728`) 最多顺序两次请求，各自 `.timeout(request_timeout_sec.max(1))` (`:769`)，默认 60 秒（`config.rs:1632`）。但每次网络 deadline 之前还有 `ResponseAudioBudget::reserve(ERROR_BODY_BYTES).await` (`:764`)，其全局 semaphore 获取 (`modules/delivery/audio.rs:98–115`) 没有期限，所以不能报告“整个 apply 至多 120 秒”。这是已有内存背压边界，本轮不删除或放松它。

TTS 远程 setup 错误目前被 warn 后继续，返回已应用键；这不是新引入的错误语义。取消发生在远程请求期间，不能证明远端未改权重，也不能撤销本地已更新配置。`ExecutionToolRegistry` (`modules/execution/registry.rs:56`) 亦是先应用执行政策，再 await raw registry；其 `ExecutionService::apply_settings` (`service.rs:497`) 先 catalog 写锁再 policy 写锁，而 catalog 读侧可跨工具 proposal 的 SQL (`:259–369`)。这些均说明对完整 apply 直接 timeout 并称回滚不成立。

## 取消所有权与持久化边界

可以安全丢弃的“准入前等待”是尚未取得 update gate，或已取 gate但尚在纯 preview/最终账户核验且未提交请求政策、desired 或模块变化的阶段。drop 会释放 Rust/Tokio guard；先前独立 current 的到期政策变化仍然保留。既有 Registry 方法取锁后的循环无 await，不会因普通 async cancellation 在循环中途停止；不由此承诺 panic、OOM 或进程 kill 的原子性。

SQL 事务 drop 由 SeaORM/SQLx 安排 rollback，不是立即完成的同步锁释放证明。本地锁定 SeaORM `database/transaction.rs:196` 明确 rollback 可排队到下次异步操作/连接回池，`:234` Drop 调用 start_rollback。commit acknowledgement 附近取消仍是“不确定是否已提交”；只能 reload/query，不能回写旧版本作全局撤销。

一旦 SQL CAS、desired 或任一模块已生效，就保留上轮已确认的部分成功与显式重试。`api/settings.rs:154` 使用 changed 加上 requested keys，使同值重试仍应用/保存；这不是自动恢复作业。文件 `settings/storage.rs:86` rename 前失败清理本次临时文件，rename 后保留完整新文件并报告持久性未确认；同步调用没有被移到 `spawn_blocking`，所以当前无“timeout返回但保存task仍在后台改盘”的新所有权分支。它仍会在慢文件系统上占用执行器线程。

客户端断开与 handler future 已取消必须分开证明。`bootstrap/runtime.rs:263–271` 使用 axum serve + graceful shutdown，Settings handler 本身没有 select 客户端断开或 state.cancel_token；不能仅 `reqwest` client task abort 就声称 server gate 已释放。服务关闭原有 drain (`runtime.rs:124`，默认 1 秒) 后关闭已接收连接并继续 join；总 cleanup deadline 默认 10 秒并保留未完成清理 owner。这些是关闭机制，不是 Settings 正常运行的总时间预算。

## 最少必要验证与 R05 门槛

现有 14 项 GET/PATCH 身份用例覆盖真实 policy/preview/apply 屏障、2 秒身份查询、拒绝无泄露、原部分成功；Settings recovery 用例覆盖模块部分失败、保存失败及同值重试。`settings/storage/tests.rs:269` 覆盖第二个 save 的准入后快照和持锁顺序。它们没有把“gate 等待取消”“handler 真正 drop 后释放”“客户端断开后服务端完成/清理”逐一实证；测试 fixture 的 abort 清理代码本身不能代替这些断言。

建议仅保留以下局部验收，不为本轮增加生产 hook 或全面系统权限审计：

- 直接拥有 in-process 路由/handler future，在既有受控 preview 屏障处取消它，观察具体 future drop、gate 可重新取得，desired/live/文件不变。另以 Registry gate 已持有的局部 future poll/drop 验证排队取消；不要用任意 sleep 假造 HTTP 已认证入队见证。
- 使用既有受控 module 在明确进入 apply 后挂起；此时取消所拥有的 handler future，核对 gate 释放、desired 保留且尚未保存的文件保持，再以同值新请求完成修复。这个测试应保护部分成功，不期待跨模块回滚。
- 需要确定客户端断开语义时，另起真实 HTTP，先见证受控 preview/apply 已进入，再关闭客户端，并观察服务端的明确结束/drop信号或释放屏障后正常完成；记录实际行为，不能从客户端 join 结果推断 server 已取消。
- R05 前做一次本地慢响应服务的在途 LLM + Settings PATCH + 后续 GET/隐私 PATCH 故障演练。要求明确可接受的管理恢复上界、可靠释放/重试方式及停止后的零遗留任务/连接；若当前 guard 跨网络造成超出验收上界，先单列窄范围锁快照/预算修复并验证，再声称 R05 长稳通过。不得以整请求 timeout 隐藏已经提交的政策和部分模块变更。

这轮先完成审计记录，后续主要投入 R02 AEC/R03 反馈的硬件资料、测量与证据。现有软件权限/文件原子性保障不被上述可用性待验项推翻；真实部署文件系统阻塞、24 小时运行、客户端取消和硬件断电没有在本分工执行，R05 仍须独立验收。

Root 当前验证与阶段进度见 [最终交接](root-HANDOFF.md#root-最终收口)，当前结果不回写为该分工独立执行。
