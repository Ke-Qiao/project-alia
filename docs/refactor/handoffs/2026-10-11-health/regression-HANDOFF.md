# Health 回归 HANDOFF

日期 2026-10-11，基线 `d27490f`。本 subagent 先只读检查既有 `/health` handler、ModuleRegistry、Router、Provider、真实 HTTP fixture、消费入口和 management checklist，再按 Root 确定的契约补回归。没有执行 Cargo、数据库、Docker 或 Git，没有修改生产文件。

## 已确认契约与复用

保留公开 `/health` 的 HTTP 200 和 `{status, version, modules}`，不增加查询参数或第二种诊断入口；OOBE 的 setup `/health` 是另一服务。现有消费者包括前端 15 秒轮询与服务器探测、discovery、CLI 操作文档及部署 smoke；没有修改前端或操作权限。

Root 确定每轮 6 秒绝对预算、最多 8 个并行检查、同 Registry 同时一轮；重叠请求立即返回原模块 keys，每项 `degraded / health check already in progress`，不排队、不缓存。超时只替换未完成项为 `health check timed out after 6s`，停止只替换未完成项为 `server is stopping`；开始前已停止则整轮拒绝且零 probe。已完成结果保留。OpenAI 原 5 秒诊断和默认 `health_probe_enabled=false` 保持。

配置中同 profile 的多个 LLM 角色应共享真实实例，初始化、start、settings、stop 每实例一次；显式注入的同名不同实例保持独立。Router 只检查本地配置与绑定，不再次探测 minor Provider。

只改 `packages/alia-server/tests/server_runtime/health.rs`，保留现有 feature 门与父模块接线，复用原 `with_fixture`、独立 PostgreSQL schema、ServerBuilder、真实 HTTP 和生命周期模式。网络请求只访问临时 loopback TCP peer；不调用外部模型、不新增依赖。

## 三条实际 HTTP 回归

### `shared_openai_health_is_opt_in_single_probe_and_busy_requests_do_not_queue`

分别用 probe=false 和 probe=true 的独立 runtime fixture，四个 LLM role 均配置为同一 OpenAI profile，并断言四者实际 `Arc::ptr_eq`。

- 默认 false 不启动本地 peer 接收任务；真实 `/health` 返回各 role 的 probe disabled，Router 本地健康。随后将 Tokio listener 转为保持 nonblocking 的 std listener，`accept` 必须返回 WouldBlock，证明本轮没有连接排入队列。此处不使用 sleep 猜测。
- true 由真实 factory 创建 OpenAiLlm。peer 读取完整 GET 路径及 Authorization 后发 Semaphore 阶段通知，不返回响应头，使真实 Provider 走原 5 秒 deadline。
- probe 进行中，Registry settings 同时更新 Provider 和 Router；500ms 内完成，profile key 只出现一份应用报告。此断言证明不跨网络 await 持有阻塞 settings 的锁，且同实例没有重复应用同一更新。
- 四个并发 `/health` 在 2 秒内全部返回 HTTP 200、原 keys 和逐项 busy，不等原 probe，peer GET 总数仍为 1。
- 原请求约 5 秒返回，各 alias 保留同一 OpenAI timeout message；正常 STT/TTS、memory/conversation 结果存在，Router 本地健康。返回内容不得含测试 key、endpoint 或 Authorization。
- 将本地 peer 改为 200 后发起下一轮，四 role 均健康，累计 GET 恰好 2，证明没有沿用上一轮失败缓存。

### `http_health_has_one_total_budget_and_preserves_independent_same_name_instances`

用现有 Builder 注入两个同名但不同实例的 LlmAdapter；健康检查各有 entered 信号，进入后保持 pending。必须先观察到两者均进入，证明至少两个检查并行，不能把顺序累计当总预算。

真实 `/health` 在 6 秒总预算结束，两个未完成项均带统一超时信息，正常 STT/TTS/Router 项保留。探测 guard 的 active 计数在 HTTP 回应前归零，证明超时丢弃了等待 future。两个实例 init、start、settings 各执行一次；结束时 stop 各一次。同名没有造成合并。把后续检查改为立即健康后，新一轮真实 HTTP 重新检查两个实例，各自累计 calls=2。

### `stopping_http_health_drops_active_probes_before_runtime_join`

观察到真实 HTTP 的 injected probe 已进入后才取消 runtime token。HTTP 必须在 2 秒内返回该未完成项 `server is stopping`，active 归零，不等 6 秒预算。停止后的直接 Registry health 全部保持 stopping，calls 仍为 1，证明没有重新 admission。最终 runtime join，stop 恰好一次且 active=0。

所有测试断言用 catch_unwind 包装；失败路径也取消并有界 join runtime，真实 TCP peer 及其 JoinSet 同样由 stop token 收口。只有正常有界 join 返回才算完成；join 超时会 abort 并等待句柄后让断言失败。

## 覆盖分工与边界

精确的 8 个并行上限、第 9 个尚未启动、别名映射、dropping aggregator future 释放 gate 等由 Registry agent 的内部测试覆盖；本 HTTP 夹具不通过注入无关模块凑足 9 个检查。生产 Provider 401/404/refused 的既有直接适配器回归保留，HTTP 组不重复它们。

默认策略、工具可见性、Execution 批准和 Shadow 不在本文件改动。真实 HTTP 只证明诊断协议与资源生命周期，不证明模型推理可用、物理播放、Pi 性能、设备在线或外部 API 实况。被测 pending future 是合作式 async；不能据此证明任意阻塞原生线程能够被强制抢占。

## Checklist 与恢复

- [x] 只读核对 API、消费者、管理端 deferred 项和已有 runtime/probe 测试。
- [x] 保留 API、默认 false 与已有 OpenAI 5 秒诊断语义。
- [x] 真实 HTTP 同 profile 去重、busy 不扩张请求与下一轮 fresh。
- [x] 同名不同实例、总预算、正常项保留与 pending future 清理。
- [x] stop 期间 HTTP 有界返回、停止后拒绝新 probe、runtime join。
- [x] 单文件 rustfmt 与静态自审完成，无新增依赖或生产修改。
- [x] Root 在统一 V2 冻结输入上完成默认完整回归（含本轮三项）及无默认全目标检查。
- [x] Root 同源验收、资源清理、checklist／恢复点和正常提交。

实现交接时本分工为 **6/8**，现经 Root 验收收口为 **8/8**。三组回归由 Root 在同一 V2 冻结输入上统一运行，本 subagent 未单独执行 Cargo；实际结果见下方最终补记。

## Root 最终验收补记

源码 `29420a57d6558d90c36098c061655dff40eb72f4`，V2 manifest `9902508f7743bbbae5d447fec76c753d9f8910b0b1b019ef3ea5f801d03a21b0`，646/646 Git blob 一致。

macOS workspace **1094/0/5**；Linux ARM64 Server 默认并行完整回归 **812/0/5**。两端无默认 feature 全目标检查、边界正反自测、变更文件格式检查和计数器 8 例自测全部通过。数字依次为 passed/failed/ignored，嵌套子进程摘要不重复累计；所有最终阶段同源、无源码漂移。

V1 两端首次编译均因聚合 stream 内 async mapper 的高阶借用生命周期泛化失败而 exit 101，没有测试通过数字。确认两端退出后，V2 仅把 mapper 改为先构造借用 Future Vec 再进入 buffer_unordered，期限、并发、取消和测试逻辑均未改变；两个版本清单与原始日志全部保留。

所有 Cargo 阶段结束；关闭前其他数据库连接为零，最终测试 schema 为零（遗留 schema 清理数 0）。V1/V2 Linux holder 与任务 PostgreSQL 已停止，用户 5433 未访问。空闲 holder 的 sleep 容器停止码不作为测试结果；V2 验证命令均 exit 0，V1 编译失败 exit 101 另行保留。缓存与只读快照保留在 ignored `run/validation-20261011-health/`。

所有 Arc 身份断言在 V1 冻结前已移入既有 catch／stop_runtime 保护区域；运行通过仍以 V2 完整日志为准。上文“尚未运行”描述 subagent 交接当时状态，此处为后续实际验收。

未运行 Linux 全 workspace、release/native release link、部署 smoke、真实 PTY、外部或付费模型、Pi/声学/执行器及长稳验证。6 秒预算约束合作式异步检查，不能强制中断同步阻塞；本地取消不代表远端立即停止处理请求。
