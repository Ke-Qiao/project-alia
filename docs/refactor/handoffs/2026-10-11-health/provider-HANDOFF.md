# 2026-10-11 Health — Provider / Router HANDOFF

## 范围与状态

基线 `d27490f`。本分工先审计现有实现，再清理 Router 重复 Provider 探测和 Gemini 探测期限／锁／诊断信息缺口。Root 负责聚合、同 profile 实例共享与生命周期、完整验证及提交；本分工不运行 Cargo、数据库、Docker 或 Git，不连接外部 Provider。

- [x] 追踪 OpenAI、Gemini、Router health、配置、热更新注册及调用者。
- [x] Router health 改为本地模式与依赖绑定状态，删除二次 Provider 探测。
- [x] Gemini 探测使用配置快照、独立 5 秒请求期限及安全错误类别。
- [x] 新增真实本地 peer、默认不联网、HTTP 状态、取消、热更新回归。
- [x] 仅对归属的两个 Rust 文件执行 scoped rustfmt。
- [x] 与 Registry agent 互做只读交叉审阅，未发现阻断缺口。
- [x] Root 完成 V2 冻结源码的 macOS / Linux 默认回归与无默认 feature 全目标检查，结果见最终补记。
- [x] Root 更新总 checklist、恢复检查点并完成源码提交，记录随本轮文档提交收口。

## 审计结论与实现

### Router

文件：`packages/alia-server/src/modules/interaction/routing/service.rs`。

旧 `health_check` 在持有 `state.read()` 时调用 `minor_llm.health_check().await`。这既会重复 Registry 的 minor 探测，也会在网络停滞期间阻塞 `apply_settings` 需要的写锁。

现健康结果只反映 Router 本地配置及依赖绑定：MajorOnly 为 Healthy；需要 minor 但未绑定时，仍返回原 `minor LLM unavailable; routing falls back` Degraded；已绑定 minor 为 Healthy。方法取得读锁后没有其他 await。Provider 当前在线状态由 Registry 的对应 `llm.*` 结果单独报告，因此 Router Healthy 不表示 Provider 在线或模型生成成功。未改分类、fallback、tool selection 或 minor / major 模式政策。

唯一生产调用者是 `ModuleRegistry::health`；既有 Router 单元测试也直接调用。`route` 和 `select_tool_candidates` 原本已经克隆 state 后再做网络等待，保持不变。

### Gemini

文件：`packages/alia-server/src/providers/llm/gemini.rs`。

旧实现持有配置读锁跨 GET 网络等待、未设置请求期限，并把原始 reqwest 错误放进公开健康响应。

现 `health_check` 先克隆配置并释放锁，显式启用探测后 GET 原 models endpoint，单次请求固定 5 秒 timeout。缺 key、probe disabled、HTTP 成功与失败的健康级别及既有消息保持。传输异常分为 `Gemini probe timed out after 5s`、`Gemini probe connection failed`、`Gemini probe request failed`，不回显 URL、API key、headers、响应 body 或原始 error。Connection failed 是传输连接错误类别，不声称只表示 TCP refusal。

Provider 没有新增后台任务、缓存、重试或新依赖；调用者取消时直接 drop 当前 health future，由请求传输释放连接。5 秒仅是本 Provider 请求期限，整个 `/health` 的聚合预算由 Root 的 Registry 分工实现。

### 已存在且保留的边界

OpenAI 已有配置快照、5 秒期限、默认 probe=false 和脱敏测试，因此本轮不改 OpenAI 文件。`health_probe_enabled` 在 `config.rs` 默认 false，仅在构造时读取；Settings Registry 没有注册此热更新键，本轮没有新增开关或改变默认政策。Gemini 原有 `apply_settings` 支持 key、base URL、模型和采样参数，保持其契约。

Builder 原来对各 role 分别构造并初始化，即使 role 配置指向同一 profile 也不是同一 Arc。因此只按 Arc 指针去重不能覆盖原有同 profile 配置；已将证据交 Root，由 Root 统一处理真实实例共享与 start / stop / apply_settings 生命周期。本人未修改 Builder、Registry 或它们的测试。

## 新增回归（源码已完成，运行结果待 Root）

本分工新增 **6 项测试（Unix）**：Router 1 项、Gemini 5 项。非 Unix 缺少 Unix-domain connection-refusal 一项，不能照抄 Unix 数量。

| 测试 | 验证范围 |
| --- | --- |
| `router_health_checks_local_binding_in_each_live_mode` | 通过真实 `apply_settings` 切换 Both / MinorOnly / MajorOnly；bound Provider 的 health fixture 直接 panic，确认 Router 从不二次探测。既有 concrete Router 测试继续覆盖未绑定 minor 的 Degraded、MajorOnly Healthy 和非法双禁用原子拒绝。 |
| Gemini `health_probe_default_disabled_and_missing_key_do_not_connect` | 本地监听器观察缺 key 和默认 disabled 都不建立连接；保持原健康级别与消息。 |
| Gemini `health_probe_preserves_http_status_without_disclosing_request_or_body` | 真实 GET 本地 peer，验证 models prefix 归一化及测试 header；200 / 401 / 404 分类正确，不泄露 key、URL path、header 或 body。 |
| Gemini `stalled_health_probe_times_out_without_blocking_settings_updates` | 完整收到请求后保留 socket 且不给响应；500ms 内热更新 key 和 model；真实约 5 秒期限返回安全 timeout 消息；下一请求使用更新后的 key / model 并成功。 |
| Gemini `cancelled_health_probe_releases_its_stalled_connection` | peer 收到完整 GET 后，作用域退出 drop pending health future；2 秒内由 peer 观察 EOF，确认取消释放连接，不等满 5 秒。 |
| Gemini `health_probe_connection_refusal_is_distinct_and_redacted` | Unix 下保持 bind 未 listen 的独占 socket，先确认内核 ConnectionRefused，再通过真实 reqwest 返回固定 connection-failed 类别；作用域回收路径。 |

所有新增 peer 均在进程内用本地 TCP 或 Unix socket，不调用真实 Google / OpenAI 服务。测试采用有界 timeout 和作用域资源回收，不 spawn 无主任务。已有 OpenAI 实际 timeout／热更新回归可继续验证聚合改动。

建议由 Root 在冻结源码后执行既有 workspace / Server 默认测试；Gemini 属默认 `llm-gemini` feature。专项目标可使用 `--lib providers::llm::gemini::tests` 与 `--lib modules::interaction::routing::service::tests`，但以 Root 全量命令和实际日志为准，不将建议命令计作已通过。

## 证据边界与后续

尚无本轮运行结果；scoped rustfmt 成功不等于编译或测试通过。未实测外部 Provider、付费请求、Pi、硬件、非 Unix refusal 或非默认跨平台网络环境。原生成请求的超时与配置锁生命周期不属于本轮 health 清理，未改。若要后续支持动态 probe 开关，应先定义 Settings Registry 与授权语义，不以本轮代码推导已有支持。

交叉审阅确认：Registry 聚合用 try_lock 拒绝重叠请求、不排队或复用缓存；原始 deadline 覆盖运行及排队检查；同实例别名共享结果，Provider 名称相同的不同实例仍各自探测；取消优先、已完成结果保留、借用 future 在 admission guard 释放前析构。Builder 的 profile map 和 `initialize_llm_role` 对同配置 profile 复用已初始化 Arc、显式 override 保持独立，RuntimeCleanup 与真实 Server runtime cancel 路径相连。上述均是静态审阅，等待 Root 测试证据。

期限以模块 future 合作式让出为前提，不强制中断同步阻塞代码；drop 当前 HTTP probe 可以关闭本地连接，但不能证明远端已经停止工作。本轮不扩展为隔离执行框架。

### 真实 HTTP 回归交叉审阅

只读审阅 `tests/server_runtime/health.rs` 的三项场景：公开 `/health` 实际 HTTP 200 与原 status / version / modules 结构；默认 probe=false 时保留监听器直接断言没有连接；四 role 通过 Builder 共享同一真实 OpenAI Arc，停滞请求只产生一次 models GET；四个重叠 HTTP 请求有界返回 busy、模块 key 集合保持，随后新请求重新探测；两个同名显式 override 保持独立生命周期、约 6 秒聚合预算后释放 future 并保留完成结果；真实 HTTP 停止中的请求 2 秒内完成，停止后的 Registry 不再准入。此 review 不把受控 GatedLlm fixture 等同真实外部模型，也不把静态阅读计作测试通过。

普通断言失败由 inner catch 捕获，先 cancel / join runtime 再恢复 panic；本地 peer 用取消 token 和 JoinSet 收拢 socket handler，外层失败路径也取消并 join peer。已向 Root 与测试作者报告一处小清理空窗：shared / independent 场景的 Arc identity 断言初版位于 runtime build 后、inner catch / stop_runtime 前，若这些断言正好失败，Runtime Drop 只启动 cleanup owner，未先等待就进入 fixture cleanup。建议只移入既有 inner catch；修正与运行结果由 Root 统一确认。

## Root 最终验收补记

源码 `29420a57d6558d90c36098c061655dff40eb72f4`，V2 manifest `9902508f7743bbbae5d447fec76c753d9f8910b0b1b019ef3ea5f801d03a21b0`，646/646 Git blob 一致。

macOS workspace **1094/0/5**；Linux ARM64 Server 默认并行完整回归 **812/0/5**。两端无默认 feature 全目标检查、边界正反自测、变更文件格式检查和计数器 8 例自测全部通过。数字依次为 passed/failed/ignored，嵌套子进程摘要不重复累计；所有最终阶段同源、无源码漂移。

V1 两端首次编译均因聚合 stream 内 async mapper 的高阶借用生命周期泛化失败而 exit 101，没有测试通过数字。确认两端退出后，V2 仅把 mapper 改为先构造借用 Future Vec 再进入 buffer_unordered，期限、并发、取消和测试逻辑均未改变；两个版本清单与原始日志全部保留。

所有 Cargo 阶段结束；关闭前其他数据库连接为零，最终测试 schema 为零（遗留 schema 清理数 0）。V1/V2 Linux holder 与任务 PostgreSQL 已停止，用户 5433 未访问。空闲 holder 的 sleep 容器停止码不作为测试结果；V2 验证命令均 exit 0，V1 编译失败 exit 101 另行保留。缓存与只读快照保留在 ignored `run/validation-20261011-health/`。

所有 Arc 身份断言在 V1 冻结前已移入既有 catch／stop_runtime 保护区域；运行通过仍以 V2 完整日志为准。上文“尚未运行”描述 subagent 交接当时状态，此处为后续实际验收。

未运行 Linux 全 workspace、release/native release link、部署 smoke、真实 PTY、外部或付费模型、Pi/声学/执行器及长稳验证。6 秒预算约束合作式异步检查，不能强制中断同步阻塞；本地取消不代表远端立即停止处理请求。
