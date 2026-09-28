# R04 设备 REST 与 OpenAI 健康探测交接 — 2026-09-28

状态：冻结 V4 完整软件验收已通过并提交 `8f5bb83`；633个已提交输入与最终manifest逐项相同。Provider新增3项、HTTP/PG新增3项均包含在1053项workspace通过中。真实PTY3、实际模型2另行通过，两平台编译及fmt通过；详见末尾回填和整包摘要。以下保留V2/V3失败及未确定根因的准确边界。

当前 agent 未运行 Cargo / PostgreSQL、未提交 Git、未调用外网或付费 Provider；本次交接仅编辑本 canonical 文档，不改变冻结源码。最终计数以 Root 的 R04 报告和对应 command/result JSON 为准。

范围依据：[R04 readiness 第 3 项](../2026-09-28/r04-readiness.md) 及随后对 `api/devices.rs` 的限定调用链核对。没有修改 registry 的健康聚合方式，也没有扩展到整个 channels API。

## 已实现：显式 OpenAI live health probe 有限完成

文件：`packages/alia-server/src/providers/llm/openai.rs`，以及 `packages/alia-server/tests/server_runtime/health.rs`。

- [x] 真实 `OpenAiLlm::health_check` 的 `GET /models/{model}` 请求设置独立 5 秒 request timeout；chat / streaming 请求期限不受此次修改影响。
- [x] 网络等待前复制配置并释放 RwLock 读锁，使 stalled probe 不阻塞 Provider settings 的写锁。
- [x] 保留默认 `health_probe_enabled=false`；缺少 API key 仍为 Unhealthy，配置 key 但禁止 live probe 仍为 Degraded，不能据非空 key 声称在线 Healthy。
- [x] 成功 HTTP 状态返回 Healthy；401 / 404 等失败保留原 HTTP status；传输错误区分 `timed out after 5s`、`connection failed`、`request failed`，均返回带 profile 的 Degraded。
- [x] 新诊断不拼接原始 reqwest error、URL、Authorization 或响应 body，避免这些字段进入健康响应。连接拒绝使用 `connection failed` 类别，与 HTTP 状态及 timeout 可区分，不声称此类别只包含 TCP refusal。

调用链：公开 `/health` → `ModuleRegistry::health` → 各 module 的 `health_check`；路由模块还可能再次检查 minor LLM。现有 GPT-SoVITS probe 的 5 秒模式是本次局部期限的参照。Registry 仍按原顺序聚合，因此 **5 秒是每次 OpenAI probe 的上限，不是整个 `/health` 的总上限**。多个显式启用的 Provider，以及 router 对 minor 的额外探测，等待时间可能累加。本次不声称为其他模块提供了新的统一总 deadline。

## 已实现：设备接口使用原 owner/admin 授权和真实状态查询

文件：`packages/alia-server/src/api/devices.rs`，以及 `packages/alia-server/tests/server_runtime/devices.rs`。

原问题是设备接口只提取 `_auth: AuthUser`，随后从全局 registry 选择 embedded channel，未比较账号与 channel owner；真正的设备 RPC 由 Server 自身凭据发出，设备侧绑定认证不能代替 REST 调用者授权。原 `/device/status` 固定返回空 ID、offline 和空遥测，不能用作诊断。

- [x] 全部 9 个 handler 经同一个 `resolve_embedded_channel`：当前 admin 或 `channel.owner_user_id() == Some(auth.id)` 才可访问；检查先于 `execute_action`，同时覆盖读取与 mutation。
- [x] 复用 `AuthUser::from_token` 的当前数据库 active / role 检查；不把 JWT 中的旧 admin claim 当作授权。账号降级、禁用之后的新请求必须按数据库当前状态处理。
- [x] 指定其他 owner 或无 owner 的设备时，普通账号返回 403；admin 可管理无 owner 的设备。
- [x] 未指定 `channel_id` 时，先筛选有权访问的 embedded channels：0 个返回 404，1 个使用该 channel，多个返回 400 并要求明确 ID，避免 HashMap 第一项决定设备。
- [x] `/device/status` 接受同样的 `channel_id` 查询，调用已有 `execute_action("binding_status")` 并设置 5 秒只读查询上限；该 action 在真实 EmbeddedChannel 中会执行 `GetBindingStatus` RPC。
- [x] 只在实际查询成功且返回非空设备 ID 时返回该 ID 与 `online=true`；错误、空白/缺少身份、超时分别返回明确 503，不合成空 ID/offline 成功响应。
- [x] CPU、内存、温度仍为 `None` / JSON `null`，因为这条契约没有可用的设备遥测来源。未以 discovery candidate 或缓存 Connected 替代当前 RPC 结果。

安全与事实边界：

- 权限是 channel 的配置 owner 或当前 admin；设备 RPC 仍沿原 Server 签名及设备绑定协议执行。本次没有新造设备权限框架，也没有更改全局 channel registry。
- `GetBindingStatus` 是设备现有公开状态 RPC。成功查询只说明当次请求可达且对端报告了身份，**不证明可信 body pin、设备实际已绑定到当前 Server、声学/硬件正常或资源遥测有效**；不能将 `online=true` 当作输出授权或 body 控制凭证。
- mDNS `discovery_status` 保持候选发现语义。Embedded 的缓存 ConnectionStatus/health 也不能单独证明此刻在线；新的 status handler 不使用它们作在线结论。
- 设备查询的 5 秒 timeout 只包围这次只读 action；不能推导为所有 mutation 都有相同总期限。失败响应使用固定安全消息，不暴露底层设备 transport 细节。

## 6 个新增测试与验收 checklist

下表列出 Unix 平台的 6 个新增测试。6项均在冻结V4全量回归中通过；前三项在Server lib，后三项在真实HTTP/PG server_runtime目标。

| 测试 | 预期验证 |
| --- | --- |
| `providers::llm::openai::tests::health_probe_default_disabled_and_missing_key_do_not_connect` | 默认禁用及缺 key 不产生 TCP 连接；配置 key 不等于在线健康，消息不含 key。 |
| `providers::llm::openai::tests::health_probe_preserves_http_status_without_disclosing_request_or_body` | 实际 adapter 请求本地 TCP，核对 model GET 与测试 Authorization；HTTP 200 / 401 / 404 保持不同健康结果，响应消息不泄露 key、URL path 或 body。 |
| `providers::llm::openai::tests::health_probe_connection_refusal_is_distinct_and_redacted` | `cfg(unix)` 下保持 bind 但不 listen 的真实 Unix-domain socket；先确认内核 `ConnectionRefused`，再由真实 reqwest/OpenAiLlm 返回安全 connection-failed 分类，与 HTTP status / timeout 分离。非 Unix 平台未实跑此类 refusal，不使用不稳定 TCP 随机端口 fallback。 |
| `health::live_openai_probe_timeout_preserves_other_http_health_results` | 真实 ServerBuilder/factory 初始化 OpenAiLlm；本地 TCP 已读取完整 GET 与 Authorization 后持有连接且不返回 header；真实 `/health` 在 7 秒测试余量内返回约 5 秒 timeout，并保留 memory、conversation、STT、TTS 结果；等待期间 settings 写锁仍可用；teardown 停止并 join HTTP runtime 与 stalled peer。 |
| `devices::device_routes_enforce_current_owner_or_admin_before_any_action` | 真实 HTTP/PG 下逐项遍历 9 个 handler：非 owner 及无 owner 的普通账号拒绝且 adapter action 计数不变，owner/admin 放行；默认 0/1/many 选择；JWT 仍为 admin 但数据库降级后拒绝，数据库禁用 owner 后认证失败，均不产生额外 action。 |
| `devices::device_status_uses_live_identity_and_reports_unavailable_without_fake_offline` | REST 返回 adapter 的实际 binding ID，区别于 discovery 候选；遥测 null；错误、空白 ID、stalled action 分别 503，不伪造 offline；stalled action 在约 5 秒、7 秒客户端测试上限内结束。 |

设备运行时测试的 channel 是带 owner、action 计数和可控状态结果的 fixture；它验证 REST 授权、选择及状态映射，**不声称这两项测试使用真实 gRPC 或真实设备**。真实 EmbeddedChannel 的既有 binding/status RPC 路径通过代码追踪确认，本轮新增 fixture 不替代既有配对或后续真机证据。

### V2 refusal 失败与 V3 测试夹具修正

Root 的冻结 V2 Server lib 实跑为 308 passed / 1 failed / 3 ignored，唯一失败是上述 refusal 测试的 2 秒外层 deadline；同冻结源码单独复核也超时。原测试先取得随机 TCP 端口再关闭 listener，既有并行复用窗口，也依赖主机及时拒绝未监听连接。随后本机 TCP 核对中，“持续 bind 但不 listen”的 IPv4/IPv6 端点仍发生等待超时，Root 另一次已关闭 TCP 端口探测则实际返回 refused。因此**具体根因未确定**，不能断言是端口复用、系统代理或某项系统网络策略，也没有修改系统策略。

V3 仅调整该测试的 Unix 平台夹具：保留独占的 Unix-domain socket 路径且从未 listen，先用 `UnixStream::connect` 断言实际内核返回 `ErrorKind::ConnectionRefused`，再用 `Client::builder().unix_socket(...)` 让真实 reqwest/OpenAiLlm 验证原 `connection failed` 分类。路径使用 `std::env::temp_dir()` 下短 UUID 文件名及作用域清理，无新增依赖。仍保留 2 秒测试 deadline、5 秒 production probe deadline 和原分类断言，没有返回伪造错误或放宽期限。HTTP 200 / 401 / 404 及不返回响应头的 stalled 场景继续使用原真实 TCP 路径。该 refusal 证据只覆盖 Unix-domain 传输上的连接拒绝，不冒称本机 TCP refusal 已稳定复现；非 Unix 平台此项没有实跑证据，也不计为通过。

原 V2 失败日志保留在 `run/validation-20260928-r04/server-lib-v2.log` 与 `health-refusal-v2.log`。V3 的 `workspace-v3.log` 明确记录上述 3 个 Provider health 测试通过及 Server lib 309 / 0 / 3；但 workspace 命令整体 exit 101，不能将该局部结果标成全量成功。Unix 平台本次仍为 6 个新增测试，其他平台不得照搬此数量。

### V3 配对超时、同 binary 复核与 V4 诊断边界

这些是 R01 已有的 scanner → actor → TTS → Server/Embedded 配对场景，并非本包 6 个新增测试。原证据均保留在 `run/validation-20260928-r04/`：

| 轮次与证据 | 实际结果 | 可得结论 |
| --- | --- | --- |
| `workspace-v3.log` / `workspace-v3-result.json`，`cargo test --workspace --locked --offline` | `duplex_playback-e65e790efe7810e2` 为旧 6 项通过、Companion 3 项均触发外层 25 秒超时；套件 35.54 秒，workspace exit 101，冻结源码无 drift。 | 三例只有统一外层 panic，当时没有阶段诊断，不能确定卡在 Ready、候选生成、短准入期限或其他等待。 |
| `paired-one-v3.log` / result JSON，单独 `-p alia-server` 精确运行 Complete | 1 / 0 / 0，测试 9.49 秒，exit 0，无 source drift；binary 是 `a8401cddf3efdf70`。 | 顶层 Server features 与原 workspace 相同，但直接依赖 fingerprint 不同，不能用此单例消除 workspace 配对失败。 |
| `paired-workspace-binary-v3.log` / result JSON，直接运行原 `duplex_playback-e65e790efe7810e2 --nocapture` | 默认线程及相同任务 PG 环境下 9 / 0 / 0，测试 13.65 秒、命令 13.69 秒，exit 0，无 source drift。 | 同一 binary 未再次复现，**不是根因已修复的证据**。 |
| V4：`tests/duplex_playback/companion.rs` | 只新增固定阶段名；外层超时后、shutdown 改变状态前，最多 1 秒读取既有 `ScanProgress`、Appraisal/Response 计数和候选/准入按 state、retirement_reason 的计数。Root已完成V4全量验证，见本文件末尾回填。 | 不改 25 秒测试上限、2 秒生产准入语义、默认线程或生产代码；只为复现提供证据，不声称修复超时。 |

本轮只读审查范围是三例共用 fixture、Companion scanner/runner、voice actor 和相关 SQL 锁/期限，没有无界扩展成全仓审计。已确认 Companion 的 quiet 是布尔策略，fixture 使用 Enabled 且 quiet=false；UTC 日界只影响午夜准入上限，本轮 08:xx UTC 失败不在日界。Live 容量 advisory lock 按 `current_schema()` 隔离，三例使用不同 schema/owner；未发现该锁让三例相互排队的证据。初始 `admit_before` 仍取原 capture 时间加 2 秒等边界的最早值，任何阶段过期会拒绝或退休原尝试，不能刷新机会重试来掩盖失败；但**尚无直接证据表明本次失败正是这条期限触发**，也不能断言是 CPU、存储饥饿、平台策略或 feature 差异。

V4 的诊断不打印用户/设备/来源身份、源文本、private grant 或原始 SQL 错误，诊断自身超时/不可用只给固定标签。后续若复现，先根据固定阶段和持久退休原因追对应路径，再决定修复；不要放宽期限、改线程数或无证据归因。若 V4 全量通过，仍保留本轮初始失败和根因未确定的记录。

- [x] 以上实现和测试源码落盘，指定文件 rustfmt 完成。
- [x] 指定文件 `git diff --check` 完成，无格式错误。
- [x] Root 已注册 `server_runtime/health.rs` 与 `server_runtime/devices.rs`。
- [x] 源码交还 Root 冻结，当前 agent 不持有 Cargo / PG 窗口，无提交。
- [x] Root 冻结 V3 workspace 已运行 OpenAI provider 测试，3 个新增 Provider 测试全部通过；Server lib 309 / 0 / 3，保留 V2 失败与夹具调整记录。
- [x] Root运行真实HTTP/PG health与devices测试，均通过；最终任务PG无非public schema和其他连接，源码前后hash一致。
- [x] 记录 V3 初始配对 6 / 3 / 0、同 workspace binary 后续 9 / 0 / 0 和尚未确定根因的边界。
- [x] V4 仅补配对超时诊断，并将指定文件 rustfmt / diff --check 后交回 Root 冻结。
- [x] Root 完成 V4 全 workspace，保存命令、exit、实际计数及 633 输入前后 hash，见末尾回填。
- [x] Root将结果计入独立R04报告，未加入R01既有数量；没有声称真机通过。

建议验证命令（本 agent 未执行）：

```sh
cargo test -p alia-server --lib providers::llm::openai::tests --locked --offline
cargo test -p alia-server --test server_runtime health:: --locked --offline
cargo test -p alia-server --test server_runtime devices:: --locked --offline
```

第一条会同时运行该模块原有测试；“新增 6 项”与命令实际总数需分别记录，不混为同一数字。使用 Root 当前任务 PostgreSQL 与验证环境，不连接用户数据库，不调用外网 Provider。


## V4 macOS 验收回填

Root 已取得完整冻结 V4 结果：workspace **1053 passed / 0 failed / 5 ignored**，319.39 秒，633 输入前后哈希无漂移。Server lib 309/0/3、server_runtime 53/0/0、duplex_playback 9/0/0；上述新增测试均包含其中，不另加总。真实 PTY 3 项及实际 CPU Easy-Turn 2 项另行通过，合计 **1058 unique macOS tests**；Server/Embedded no-default all-targets、fmt 也通过。V3 超时仍没有已证明的根因，不因本轮通过删除历史失败。最终 Linux/提交状态以 [整包摘要](validation-summary.json) 和 [Root交接](runtime.md) 为准。

最终源码：`8f5bb83f4a313723428abf854c1e663b159c6c45`；[已提交输入核对](committed-source-verification.json)、[整包验收](validation-summary.json)。后续从 [检查点](../../resume-checkpoint-2026-09-28.md) 和 R02/R03/R05/R06 checklist 接续。
