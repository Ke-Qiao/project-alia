# Server 与 Embedded 无前端操作入口

更新：2026-10-14。本轮以当前开发 Mac Server、Pi 5 / 8 GB Embedded 为目标，Easy-Turn 位于 Server。Pi 的 Ubuntu24.04.4 / aarch64 已经只读核对；实际音频链、执行器接线/校准、服务目录和模型服务仍待现场资料。以下指向当前 CLI/REST 和既有权限契约；Web/iOS/Android 不参与安装或验收。

当前硬件/部署输入、只读核对与R02/R03→R05门槛集中在 [部署资料清单](hardware-deployment-readiness.md)。
## 配置与启动

从仓库根目录按 [本地开发](local-development.md) 准备主文件和 `.d` 分片；[Server 配置](../../packages/alia-server/docs/configuration.md)与 [Embedded 配置](../../packages/alia-embedded/docs/configuration.md)分别管理业务/模型和设备实时运行时。`ALIA_CONFIG_PATH` 可以选择 Server 主文件，配套分片目录为同名 `.d`；Embedded 使用独立的 `ALIA_EMBEDDED_CONFIG_PATH`。

正常启动会产生实际副作用。Embedded 可能创建或调整设备密钥权限、保存配置、创建/迁移 SQLite、绑定监听地址，然后在 `Builder.build()` 返回前启动选定的播放、motion、camera、capture 和 discovery。未配对不代表麦克风尚未开始采集；`motion.hardware_enabled=false` 只限制执行器，不能关闭其他设备。通用 [embedded.example.toml](../../config/embedded.example.toml) 是配置说明示例，其中原生采播、模型、vision 和发现仍按模块选择启用，不能直接当本轮 Pi 现场预设。

已有二进制提供独立的只读检查入口，必须指定实际存在的配置文件：

```sh
./target/debug/alia-embedded --check-config /absolute/path/embedded.toml
```

此入口只读配置，以及配置指向的校准文件和已批准动作文件，执行静态验证；不创建/调整 key，不进入 Builder、不打开数据库、模型、PCM、camera、串口或 I²C，不执行绑定、网络发现或启动服务。缺少配置明确报错，不将 compiled defaults 当作已检查的文件。报告指出配置选择与前置条件，不能证明真实设备、模型、机构安全或目录写权限已经可用，也不核对 Server 的批准清单。构建二进制本身会写入构建目录，应与这次只读运行区分；不要用普通 `cargo run` 或视觉 setup 脚本代替预检。

如需后续验证无设备的正常生命周期，使用专门的 [embedded.staging.example.toml](../../config/embedded.staging.example.toml)，先复制到新的私有配置目录，再检查复制后的文件。模板显式关闭全部 module 槽位、motion、vision/debug、毫米波和 mDNS/BLE，监听 `127.0.0.1:0`，保留正常绑定边界；state、key、plugin、cue 使用相对配置文件的独立 `staging/` 路径。不要与已部署设备共用这些目录。正常 staging 启动仍会写 key/state/config 并创建 loopback listener，**不属于只读操作**；它也不检验真实录放音或执行器。

```sh
ALIA_CONFIG_PATH=/absolute/path/server.toml cargo run -p alia-server
ALIA_EMBEDDED_CONFIG_PATH=/absolute/path/embedded.toml cargo run -p alia-embedded
```

上面是正常运行命令，现场前置未完成时不执行硬件配置的启动。staging 需要正常运行时，应把第二行路径明确指向复制后的 staging 文件。使用该前台进程的 Ctrl+C 或针对其 PID 的 SIGTERM 请求正常关闭，并核对进程退出与清理结果；超时不证明 native 工作或物理输出已安全停止，不使用按名称批量 kill 或顺带停止共享服务。

仓内 `pnpm dev:backend` 会同时启动开发数据库、Server 和 Embedded，`pnpm dev` 还包含前端；这些便利入口不负责本轮隔离目录、设备权限或测试资源归属。`scripts/setup-embedded-vision-pi.sh` 是安装/模型/摄像头配置工具，可能安装依赖、下载文件和查询设备节点，`--run` 会启动完整 Embedded；它不是无设备只读预检。Server 普通配置加载也可能执行已有发布恢复；不能把正常 Server 启动称为纯只读解析。

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

`save-config` 的正常 JSON 响应包含 `success`、`path`，失败时另有 `error`。配置验证或保存失败也可能返回 HTTP 200，因此必须检查 `success`；仅用 `curl --fail` 不够。发布开始前的失败允许修正后重试。发布已开始但同步失败时，错误会明确持久化仍为 unknown；setup 保留原发布，只接受同一路径、内容和安装方式的重试，不能换请求覆盖。只有持久化确认后才进入 Ready；成功响应排空后 setup 关闭，其端点不作为常驻管理 API。准备阶段的 `/health` 返回 `status: "setup"`，不是模块健康结果；启动完成后使用配置中的实际 Server 地址和端口。

同一 setup 进程只有一次成功完成：并发 HTTP/CLI 的输家返回失败且不写第二份配置；取消后，即使已接收请求随后才补齐 body，也不会发布。base、分片树及完整 canonical 祖先目录链按发布协议同步；若发布已开始后失败，不删除 base 或 CLI 分片，而是保留私有的相邻恢复记录。记录只含路径和内容摘要，不含配置值或密钥。CLI 按 `c` 重试原安装；HTTP 重发原请求；取消保留磁盘文件。重启时选择相同配置路径（自定义路径用 `ALIA_CONFIG_PATH`），加载器会先恢复再加载，正常无记录时仍纯只读。

恢复发现 base、候选或分片内容改变/丢失时会拒绝盲覆盖，应保留文件与报错交由操作者核对，不要只删恢复记录强行继续。恢复记录建立前崩溃可能留下需人工核对的临时文件/CLI 分片；也不能同时让其它进程修改正在校验的文件树。实际断电、存储硬件与服务账户部署权限仍需独立验证，见 [本轮发布与恢复说明](../refactor/config-durability-2026-10-11.md)。

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
| 配置/模块健康 | `/health`、`GET /api/v1/settings` | 检查每模块状态；默认 probe=false；OpenAI/Gemini 在线 GET 需显式启用、单次 5 秒。整体一轮最多 6 秒、8 项并发；busy/disabled 均不能当作在线通过。 |

`GET /health` 保持公开 HTTP 200 和 `status`、`version`、`modules` 响应字段，仍需检查各模块状态。相同配置 LLM profile 的多个角色共享一次检查；Router 只报告本地模式和依赖绑定，模型状态见对应 `llm.*`。同一 Registry 有一轮检查进行时，重叠请求立即返回 `degraded` / `health check already in progress`，不排队、不复用旧结果。6 秒整体期限到达后保留已完成状态，未完成项返回 `degraded` / `health check timed out after 6s`；停机时未完成项返回 `server is stopping`，单项 panic 返回 `unhealthy` / `health check panicked`。后续请求在空闲时重新检查。

这些期限约束合作式异步检查；Server 不启动后台探测，也不能保证已发出的远端请求在远端立即停止。设置更新不等待健康网络探测，已经发出的探测使用其开始时的配置快照。没有新增深度诊断端点或 probe 热切换开关。

四个 Companion 操作和 execution/delivery 契约可在 Server `/scalar` 查阅。其他旧模块的 OpenAPI 覆盖不应由这个页面存在就自动推断；对应 Markdown 和实际 REST 用例仍是操作依据。

`GET /api/v1/settings` 在全部设置快照等待结束后、返回 JSON 与政策 ETag 前核验当前 active admin。等待期间已降权、停用或删除为 403；最终核验失败或超过 2 秒为脱敏 503，均不返回设置或 ETag。成功响应仍保留原值/schema 与 prefix 行为，包括管理员可读取的 secret 值。原 SQL 暂停到期与内存政策投影保持，响应准入后不持续持有用户锁。

纯非政策 `PATCH /api/v1/settings` 会在更新锁、政策读取和 embedding 预览结束后，重新核验当前 active admin。等待期间已降权、停用或删除返回 403；核验数据库失败或等待超过 2 秒返回脱敏 503，且不应用或保存本请求设置。已经准入的操作继续沿用部分成功规则，不因随后降权自动回滚。

运行时 `PATCH /api/v1/settings` 有 SQL 政策提交、期望设置更新、模块生效和文件保存几个独立完成点。HTTP 200 中 `success=false` 不表示此前动作已回滚，应核对 `context_policy_version`、`applied`、`module_reports` 与 `errors`。同值显式重试仍会调用模块并保存。`settings.toml` 通过完整快照原子替换；发布前失败保留旧文件，发布后同步失败明确报告持久性未确认。失败后直接重启只加载磁盘上完整的旧版或新版，SQL 仍是政策权威；此路径没有首次 setup 的恢复记录。详见 [运行时设置说明](../../packages/alia-server/docs/configuration.md#runtime-settings)。

`PATCH /api/v1/memories/privacy` 现要求管理员，提交同一全局 SQL 政策并支持相同 `If-Match`；普通登录用户仍可读取隐私状态。隐私 GET 与 audit 读取实际政策，不再返回旧内存投影。该权限收敛已由用户确认。

绑定请求应沿设备实际发现结果填写 `channel_id`，保持 `require_confirmation=true`。终端操作不应绕过本地配对确认；这项软件接口检查仍不等于真实 Pi 的麦克风/声卡和执行器已经验收。

## 诊断与恢复

ExplicitWait 只响应新的明确 Continue；临时避让可由新鲜 Backchannel 恢复。恢复使用原 Response、原 generation 和尚未提交的音频后缀，不调用新 TTS 重建前缀。协议要求 voice v2、playback v3，主动邀请额外要求 capture gate v1，见 [语音协议](../refactor/voice-protocol.md)。

数据库或网络错误可能使写入完成状态未知。读取原政策、operation、Response、Delivery 或 Effect 核对，不换身份推测性重做。`cancel_requested` 不是设备已停声/机构已到位；最终还需原 native owner 与设备事实。

本包验证使用独立临时配置、本地受控 Provider 和隔离测试数据库。实际安装、配对及 24 小时混合负载仍归 [R02—R06 checklist](../refactor/checklists/next-round.md)。Temporal/Rig/rmcp/Home Assistant 按具体用例和验证门槛接入，本包不新增这些依赖。
