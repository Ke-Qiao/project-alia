# Server 与 Embedded 无前端操作入口

更新：2026-10-04。本轮以 macOS Server、Pi 5 Embedded 为目标，Easy-Turn 位于 Server。以下指向当前 CLI/REST 和既有权限契约；Web/iOS/Android 不参与安装或验收。真机部署仍需确定 Pi 系统、音频设备及执行器。

## 配置与启动

从仓库根目录按 [本地开发](local-development.md) 准备主文件和 `.d` 分片；[Server 配置](../../packages/alia-server/docs/configuration.md)与 [Embedded 配置](../../packages/alia-embedded/docs/configuration.md)分别管理业务/模型和设备实时运行时。`ALIA_CONFIG_PATH` 可以选择 Server 主文件，配套分片目录为同名 `.d`；Embedded 使用独立的 `ALIA_EMBEDDED_CONFIG_PATH`。

```sh
ALIA_CONFIG_PATH=/absolute/path/server.toml cargo run -p alia-server
ALIA_EMBEDDED_CONFIG_PATH=/absolute/path/embedded.toml cargo run -p alia-embedded
```

未找到 Server 配置时进入 OOBE；配置已存在但不能加载时会报错退出，不自动改写成样例。stdin/stdout 都是交互终端时，输入 `c` 再按 Enter 可生成主文件加 `.d` 的完整布局，该操作拒绝覆盖已有主文件或分片树。

setup 固定监听 `127.0.0.1:3000`，此时尚未使用主配置的 Server 端口，不在局域网广播未认证的配置写入口。远程机器可使用 SSH 隧道；无交互终端时使用 HTTP setup。两个配置操作均为 POST：`generate-config` 返回 `{template: TOML字符串}`，`save-config` 接受 `{config: TOML字符串, path?: 目标主文件路径}`。`path` 指向 **Server 所在机器**的文件系统；省略时先采用该进程的 `ALIA_CONFIG_PATH`，然后按 CLI 默认路径选择。

setup 只接受 `127.0.0.1`、`localhost` 或 `[::1]` 的 Host；浏览器带 Origin 时必须与 `http://Host` 精确一致，否则返回 403。保存必须使用 JSON Content-Type，错误媒体类型返回 415。无 Origin 的本地 curl/SSH 隧道按现有方式工作；不要把入口放到任意域名的反向代理后。该策略约束浏览器来源，仍信任本机操作者，文件可写范围由服务账户权限决定。

HTTP 模板是包含所需分片内容的完整单文件，不会额外创建 `.d`；已有目标 `.d` 仍按普通加载规则覆盖主文件。保存前校验实际有效的 role/profile 引用，成功后退出 setup 并按配置启动 Server。以下命令需要 `jq`，可在第二个终端执行：

```sh
umask 077
curl --fail --silent --show-error -X POST \
  http://127.0.0.1:3000/api/v1/setup/generate-config \
  | jq -er '.template' > setup-server.toml
# 编辑 setup-server.toml，填写数据库、模型地址及密钥。
jq -n --rawfile config setup-server.toml \
  --arg path /absolute/path/server.toml \
  '{config: $config, path: $path}' > setup-request.json
curl --fail --silent --show-error \
  -H 'Content-Type: application/json' --data-binary @setup-request.json \
  http://127.0.0.1:3000/api/v1/setup/save-config \
  | jq -e 'if .success then {success, path} else error(.error // "setup save failed") end'
```

`save-config` 的正常 JSON 响应包含 `success`、`path`，失败时另有 `error`。配置验证或保存失败也可能返回 HTTP 200，因此必须检查 `success`；仅用 `curl --fail` 不够。失败不会发送 ConfigReady，setup 继续等待修正后的请求；成功响应排空后 setup 关闭，其端点不作为常驻管理 API。准备阶段的 `/health` 返回 `status: "setup"`，不是模块健康结果；启动完成后使用配置中的实际 Server 地址和端口。

同一 setup 进程只有一次成功完成：并发 HTTP/CLI 的输家返回失败且不写第二份配置；取消后，即使已接收请求随后才补齐 body，也不会发布。base 文件按原子方式发布，但父目录和 CLI 分片树尚未完整同步落盘，不能承诺断电后的目录持久性；也不能同时让其它进程修改正在校验的 fragments/symlink。详细边界见 [本次 setup 审查](../refactor/handoffs/2026-10-04/runtime.md)。

配置语义有效不等于数据库、模型或硬件已在线。先准备独立 PostgreSQL/pgvector 和选择的 [模型服务](model-services.md)，再核对 `/health`；不要以 HTTP 200 或非空 API key 判断所有 Provider 可用。

## 账号与现有管理入口

通过 `POST /api/v1/auth/register` 建立账号，`POST /api/v1/auth/login` 获得 token，后续请求使用 `Authorization: Bearer …`。首个账号的管理员分配规则以 [认证实现](../../packages/alia-server/src/api/auth.rs)为准；不要在命令历史中直接放入真实密码或 token。下列示例假定 `ALIA_URL` 和 `ALIA_TOKEN` 已指向本人的隔离开发实例。

```sh
curl --fail --silent --show-error \
  -H "Authorization: Bearer $ALIA_TOKEN" \
  "$ALIA_URL/api/v1/users/me"
curl --fail --silent --show-error "$ALIA_URL/health"
```

| 用例 | 已有入口 | 权限与结果边界 |
| --- | --- | --- |
| 渠道/设备发现与绑定 | `GET /api/v1/channels`，`GET /api/v1/device?channel_id=…`，`POST /api/v1/device/bind` | 使用明确的 Embedded channel，设备绑定要求实际配对/确认，发现不代表绑定完成。 |
| 设备状态与设置 | `GET /api/v1/device/status?channel_id=…`，`GET/PATCH /api/v1/device/settings?channel_id=…` | 设备 owner 或当前管理员；缺少遥测保留 null，网络失败不伪造成已观测的离线数据。 |
| 解绑与重连 | `POST /api/v1/device/unbind`，`POST /api/v1/channels/{id}/restart` | 重启渠道需管理员；解绑退休原 session，重新绑定使用新凭证/session，不复用旧权限。 |
| 主动策略、shadow/回放 | 四个 Companion REST 操作 | 见 [Companion 操作](companion.md)；政策完整 CAS，503 必须核对原意图，默认 shadow。 |
| 记忆/隐私/检索配置 | `/api/v1/memories/*`，`GET/PATCH /api/v1/settings` | 见 [Memory 操作](memory.md)；账号归属、来源撤销、持久 revision 不因重试刷新。 |
| 人格与关系状态 | `/api/v1/personas/me`，关系读取/清理/重建接口 | 见 [Relationship 操作](relationship.md)；清除需原 operation/revision，重建不能复活已删除来源。 |
| 工具执行/确认/取消 | `/api/v1/tools/execute`，`/api/v1/effects/{id}`，`/{id}/confirm`、`/{id}/cancel` | 从 `/scalar` 的 execution 契约取得完整请求；保留原 effect identity/version，Unknown 先核对，不再次调用非幂等工具。 |
| Response 与文本/音频/身体结果 | `/api/v1/responses/{id}`、`/{id}/deliveries`、`/{id}/events` | 见 [Delivery API](../refactor/delivery-api.md)；Submitted、Presented、位置反馈和完成分别记录，查询不是重播。 |
| 配置/模块健康 | `/health`、`GET /api/v1/settings` | 检查每模块状态；在线 OpenAI probe 显式启用、单请求5秒，顺序多个 probe 耗时可能累加；disabled 状态不能当作在线通过。 |

四个 Companion 操作和 execution/delivery 契约可在 Server `/scalar` 查阅。其他旧模块的 OpenAPI 覆盖不应由这个页面存在就自动推断；对应 Markdown 和实际 REST 用例仍是操作依据。

绑定请求应沿设备实际发现结果填写 `channel_id`，保持 `require_confirmation=true`。终端操作不应绕过本地配对确认；这项软件接口检查仍不等于真实 Pi 的麦克风/声卡和执行器已经验收。

## 诊断与恢复

ExplicitWait 只响应新的明确 Continue；临时避让可由新鲜 Backchannel 恢复。恢复使用原 Response、原 generation 和尚未提交的音频后缀，不调用新 TTS 重建前缀。协议要求 voice v2、playback v3，主动邀请额外要求 capture gate v1，见 [语音协议](../refactor/voice-protocol.md)。

数据库或网络错误可能使写入完成状态未知。读取原政策、operation、Response、Delivery 或 Effect 核对，不换身份推测性重做。`cancel_requested` 不是设备已停声/机构已到位；最终还需原 native owner 与设备事实。

本包验证使用独立临时配置、本地受控 Provider 和隔离测试数据库。实际安装、配对及 24 小时混合负载仍归 [R02—R06 checklist](../refactor/checklists/next-round.md)。Temporal/Rig/rmcp/Home Assistant 按具体用例和验证门槛接入，本包不新增这些依赖。
