# R04 CLI / REST 新装与诊断：只读核对

更新：2026-09-28。核对者：`companion_replay_28`。读取时 HEAD 为 `8d74b6c`，同时存在 R01 / CP05 未提交实现；本文件独立记录后续 R04 工作，不扩大 R01 验收范围。

依据：[next-round.md 的 R04](../../checklists/next-round.md)。本次只读源码和配置样例，没有修改生产文件、执行 Cargo / PostgreSQL、启动服务、调用付费 Provider 或更改用户政策。以下三项的代码事实已核对，动态复现和修复均未执行。Easy-Turn 已由 Root 恢复，不属于本次问题范围。

## 1. HTTP 新装模板缺少它引用的 profile 分片

**触发条件：** 全新目录没有 `server.d`，通过现有 HTTP OOBE 的 generate-config → save-config 原样保存模板。无交互终端时，`bootstrap/setup.rs:42` 明确引导用户走 HTTP setup API，因此这是已有受支持入口。

**源码事实：**

- `packages/alia-server/src/oobe/routes.rs:75` 仅返回 `config/server.example.toml`；`:113` 只做 `ServerConfig` 反序列化；`:143` 只写主文件，随后返回成功并发出 `ConfigReady`。
- `config/server.example.toml:89` 绑定 `sentence_expression = "sentence_expression"`；对应定义只在 `config/server.example.d/profiles/llm.toml:47`。`config.rs:1158` 的默认 profile 只有 minor / major / embedding，没有 sentence_expression。
- `config.rs:2226` 从同名 `.d` 目录合并分片，目录缺失不会报错；`module/builder.rs:391` 会创建 sentence_expression LLM，`module/factory.rs:51` 查询 profile，最终错误文本来自 `config.rs:1123`：`LLM profile 'sentence_expression' is not configured`。
- 现有终端生成路径 `oobe/bootstrap.rs:169` 已写出主文件和完整分片；README 的手工复制方式也包含分片。这一缺口针对 HTTP 新装路径。

**影响与证据边界：** 反序列化允许默认 profile，保存动作没有验证 role 是否可解析。按代码路径推导，HTTP 保存可以报告成功，随后构建模块才暴露缺失 profile；数据库连接、编译 feature 等更早错误可能先遮蔽它。本轮没有把这一推导写成实际启动失败记录。

**必要收口：** 使 HTTP 模板生成/保存与既有配置格式一致，并在发出 ConfigReady 前核对实际合并后的 role/profile 引用。复用已有模板和分片定义，避免维护两份默认配置。无需增加前端或配置框架。

**最小验收：** 使用独立 OOBE fixture 和临时目录；不能把运行中的用户 Server 当作 setup fixture。以下是后续窗口命令，未执行：

```sh
ALIA_R04_TMP=$(mktemp -d)
# ALIA_R04_OOBE_URL 指向独立测试实例；现有 run_setup 固定使用 3000。
curl --fail --silent --show-error -X POST \
  "$ALIA_R04_OOBE_URL/api/v1/setup/generate-config" \
  > "$ALIA_R04_TMP/template.json"
jq --arg path "$ALIA_R04_TMP/server.toml" \
  '{config: .template, path: $path}' "$ALIA_R04_TMP/template.json" \
  > "$ALIA_R04_TMP/save.json"
curl --fail --silent --show-error \
  -H 'Content-Type: application/json' \
  --data-binary "@$ALIA_R04_TMP/save.json" \
  "$ALIA_R04_OOBE_URL/api/v1/setup/save-config" \
  | jq -e '.success == true'
test -f "$ALIA_R04_TMP/server.d/profiles/llm.toml"
```

上述文件断言适用于沿用既有多文件格式的修复。必须另外在同一 HTTP 回归中调用真正的 `ServerConfig::load` 和 profile 查询，证明 `sentence_expression` 可解析，major 的 provider/model 等于完整样例，而不是悄悄回落到编译默认值。该测试应复用隔离目录，不需要联网初始化模型。错误模板必须在 ConfigReady 之前拒绝。

## 2. Companion REST 管理已存在，但发布的 OpenAPI 没有该契约

**触发条件：** 新装操作者沿 Server 提供的 `/scalar` 查阅和使用主动策略管理。

**源码事实：**

- `packages/alia-server/src/api/companion.rs:168` 已注册 policy GET / PUT、shadow POST、replay GET；`api/mod.rs:38` 合并这些实际路由。
- 同文件 `:23` 的 policy 更新要求完整 `{expected_revision, policy}`，`:74` 的未知完成结果要求读取当前政策后再处理原 revision，`:157` 映射 CAS 冲突。
- `packages/alia-server/src/openapi.rs:12` 的 paths、`:53` 的 schemas 均未登记 Companion；handler 也没有 `utoipa::path`。`bootstrap/runtime.rs:233` 使用这个 ApiDoc 提供 `/scalar`，因此它不会显示这四个操作及重试规则。
- [companion-contract.md](../../companion-contract.md) 已有 Markdown 契约，不能说 API 或规则完全不存在；缺的是现有机器可读/交互文档入口的一致性。

**必要收口：** 补齐这四个已有操作的 OpenAPI 请求、响应、Bearer owner 身份及 409 / 410 / 503 语义，并提供一段当前政策读取 → CAS 修改 → 冲突/未知结果处理的终端步骤。沿用 REST 即可，不需要另造 CLI 可执行程序或新界面。

**最小验收：** 对 `ApiDoc::openapi()` 序列化结果增加断言，检查四个方法、路径、policy 必填字段和错误分支；不要凭 `/scalar` 返回 200 判为契约齐全。现有真实 HTTP 政策回归位于 `tests/server_runtime/companion.rs:40` 和 `:705`，后续增加 OpenAPI 断言时保留这些行为回归。独立测试账号可用下列命令复核操作文档，默认测试仅写 shadow：

```sh
# ALIA_R04_URL / ALIA_R04_TOKEN 是隔离测试 Server 与账号。
ALIA_R04_POLICY_TMP=$(mktemp -d)
curl --fail --silent --show-error \
  -H "Authorization: Bearer $ALIA_R04_TOKEN" \
  "$ALIA_R04_URL/api/v1/companion/policy" \
  > "$ALIA_R04_POLICY_TMP/current.json"
jq '{expected_revision: .revision, policy: (.policy | .mode = "shadow")}' \
  "$ALIA_R04_POLICY_TMP/current.json" > "$ALIA_R04_POLICY_TMP/update.json"
curl --fail --silent --show-error -X PUT \
  -H "Authorization: Bearer $ALIA_R04_TOKEN" \
  -H 'Content-Type: application/json' \
  --data-binary "@$ALIA_R04_POLICY_TMP/update.json" \
  "$ALIA_R04_URL/api/v1/companion/policy"
curl --silent --show-error -o "$ALIA_R04_POLICY_TMP/conflict.json" \
  -w '%{http_code}\n' -X PUT \
  -H "Authorization: Bearer $ALIA_R04_TOKEN" \
  -H 'Content-Type: application/json' \
  --data-binary "@$ALIA_R04_POLICY_TMP/update.json" \
  "$ALIA_R04_URL/api/v1/companion/policy"
```

第二次原 revision 请求应为 409，且不能多写政策。第一次如返回 503，先 GET 当前政策并核对原 intent；不能把它当成肯定未提交，也不能直接换 revision 重做。这些命令不代替已有的未知结果与 owner 隔离测试。

## 3. OpenAI 在线健康探测没有超时，单个 Provider 可阻塞整个诊断接口

**触发条件：** OpenAI profile 配置非空测试 API key，并显式设置 `health_probe_enabled = true`；其服务接受连接但不返回响应头。样例默认 false，因此不是默认安装必然触发的问题。

**源码事实：**

- `packages/alia-server/src/providers/llm/openai.rs:258` 使用 `Client::new()`；`:425` 的 health_check 在 `GET /models/{model}` 的 `send().await` 前没有请求超时。
- 当前锁定的 reqwest 0.12.28 本地源码 `src/async_impl/client.rs:299`、`:313`、`:314` 的 connect/read/total timeout 默认都是 None；没有隐含的请求截止时间。
- `packages/alia-server/src/module/registry.rs:66` 顺序等待每个 module health；`api/health.rs:52` 直接等待聚合结果，Server HTTP 装配也没有提供该请求的总超时。
- 现有 GPT-SoVITS 健康检查 `providers/tts/gpt_sovits.rs:832` 已使用 5 秒超时，可作为当前诊断边界的参考。

**影响与证据边界：** 上述 stalled response 条件下，健康请求没有应用层有限完成保证，操作者不能从 `/health` 得到该 Provider 的失败类别。此结论来自实际 OpenAI adapter 调用链和依赖默认值，尚未在本轮启动 stalled server 实测；不是使用 Noop Provider 证明网络故障。

**必要收口：** 为已有 live probe 加明确的有限截止时间，错误结果指出 profile / probe 失败类别并继续返回其余组件状态。不得把非空 key 当成在线可用，也不输出 Authorization 值。是否并行聚合不属于本次必要范围。

**最小验收：** 复用现有运行时测试装配，真实 `OpenAiLlm` 指向本地受控 TCP 服务；该服务读取请求后保持连接，不返回 header，其他组件使用现有有界 fixture。测试必须观察到 probe 确实到达，然后断言配置的期限内 `/health` 返回降级结果及其他组件状态，并在 teardown 关闭持有连接。不能用连接被拒绝代替 stalled response。

独立 fixture 装配后，外部复核命令如下；若采用 5 秒 probe 窗口，7 秒留作测试余量：

```sh
curl --fail --silent --show-error --max-time 7 \
  "$ALIA_R04_URL/health" \
  | jq -e '.modules["llm.major"].status == "degraded"
           and (.modules["llm.major"].message | length > 0)
           and (.modules | has("memory"))'
```

当前实现预期由 curl 在 7 秒结束客户端等待，不能把这个客户端 deadline 记作 Server 已有限完成。后续回归还需覆盖 401 / 404 / connection refused，确认它们保留可区分的诊断，不扩展到新的 Provider 功能。

## 接续 checklist

- [x] R04 原 checklist 与三条调用链完成只读核对；未修改 R01 生产文件。
- [ ] R04-01：隔离 HTTP 新装复现、完整配置产物和语义验证，记录实际命令/结果。
- [ ] R04-02：四个已有 Companion 操作进入 OpenAPI，终端 CAS 步骤及实际 HTTP 回归。
- [ ] R04-03：真实 OpenAI adapter 的 stalled response 有界失败与聚合健康回归。
- [ ] Root 在 R01 冻结/提交后安排独立 R04 编辑与统一 Cargo 窗口；这些未执行检查不计入当前 R01 通过数。

当前进度为 **3 个必要缺口已定位，0 / 3 已修复或动态验收**。未引入新的功能范围。
