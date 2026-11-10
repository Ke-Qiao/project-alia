# Companion 政策、shadow 与回放

Server 在 `/scalar` 发布以下四个现有 REST 操作。它们使用 `Authorization: Bearer <JWT>`，每次重新核对数据库中的 active account；仅操作调用者自己的政策和证据。管理员角色也不能通过这些接口指定另一个 owner。客户端不能上传 owner、机会、预算、Context、epoch、scope 或来源原文，未知 JSON 字段会被拒绝。

| 方法与路径 | 请求 | 成功响应 |
| --- | --- | --- |
| `GET /api/v1/companion/policy` | 无请求体 | `PolicySnapshot`：正整数 `revision` 和完整 `policy`；首次读取可建立默认政策 |
| `PUT /api/v1/companion/policy` | `{expected_revision, policy}`，完整替换 | 已提交的新政策，`revision` 增加 1 |
| `POST /api/v1/companion/shadow` | `{operation_id, appraisal_ids}` | 原冻结 `ShadowReport`，包含 `operation_id/as_of/policy_revision/examined/limited/decisions` |
| `GET /api/v1/companion/replay/{operation_id}` | 原 shadow 操作的非零 UUID | 当前权限复核后的原冻结报告与纯决策回放 |

shadow 最多接收 32 个不同且非零的规范 Appraisal ID，单次报告最多 4 个决策。它只从已有规范来源读取有界事实，不运行模型、不投递声音、不占用 live 准入预算。相同 `operation_id` 和相同 Appraisal 集合返回原回执；换来源集合或碰到其他 owner 的操作 ID 返回 `409`。回放也不重播输出、发行输出权限或刷新来源期限。

## 政策字段与启用边界

PUT 的七个政策字段全部必填，没有局部 PATCH 或请求体默认值。`expected_revision` 必须为正整数，取自 GET 的结果。

| 字段 | 默认值 | 接受的值 |
| --- | --- | --- |
| `mode` | `"shadow"` | `"disabled"`、`"shadow"`、`"enabled"` |
| `quiet` | `false` | 布尔值；true 抑制主动准入 |
| `paused` | `false` | 布尔值；true 抑制主动准入 |
| `daily_budget` | `3` | 0–24，按 UTC 日期计数；未知的已准入结果也计费 |
| `cooldown_seconds` | `900` | 30–86400 |
| `candidate_ttl_seconds` | `60` | 1–300；基于规范来源的原始时间，重试不刷新 |
| `require_response_before_next` | `true` | 布尔值；true 时等待规范回应后才考虑下一次邀请 |

政策持久保存，重启不恢复为默认值。`enabled` 需要 Server 已装配 live runner，否则 PUT 返回 `400`。接受 enabled 只证明政策已保存；实际输出仍要求有效来源、预算、新鲜 VoiceSession 机会、设备能力及本地 capture gate。当前安装默认是 shadow。新建 shadow 操作在 enabled 模式下不可用，会走 `503 COMPANION_SHADOW_UNCONFIRMED`；已存在的回执仍先按原身份核对，但政策修订可能使其证据失效。

`paused` 是主动邀请政策，和正在播放声音的明确暂停/临时避让不是同一项设置。设备的 Hold、Continue 和 Cancel 沿既有语音控制执行。

## 终端读取与 CAS 更新

下面只演示把政策设为 shadow。`ALIA_COMPANION_URL` 指向操作者选定的 Server 根地址（不含末尾 `/`），`ALIA_COMPANION_TOKEN` 使用该操作者的有效 JWT；本文命令不会自动注册、登录或启用 live。需要 `curl`、`jq` 和生成 shadow 身份时使用的 `uuidgen`。排查时不要把 token、冻结报告或完整私密证据贴到公开日志。

```sh
# 先设置 ALIA_COMPANION_URL 和 ALIA_COMPANION_TOKEN，再执行。
: "${ALIA_COMPANION_URL:?请设置 Server 根地址}"
: "${ALIA_COMPANION_TOKEN:?请设置当前 owner 的 JWT}"
ALIA_COMPANION_TMP=$(mktemp -d)
chmod 700 "$ALIA_COMPANION_TMP"

curl --fail --silent --show-error --max-time 10 \
  -H "Authorization: Bearer $ALIA_COMPANION_TOKEN" \
  "$ALIA_COMPANION_URL/api/v1/companion/policy" \
  > "$ALIA_COMPANION_TMP/current.json"

# 从完整当前政策建立并保留原始意图；重试时不要重新生成此文件。
jq -e '{expected_revision: .revision, policy: (.policy | .mode = "shadow")}' \
  "$ALIA_COMPANION_TMP/current.json" > "$ALIA_COMPANION_TMP/update.json"

ALIA_COMPANION_STATUS=$(curl --silent --show-error --max-time 10 \
  -o "$ALIA_COMPANION_TMP/update-result.json" -w '%{http_code}' -X PUT \
  -H "Authorization: Bearer $ALIA_COMPANION_TOKEN" \
  -H 'Content-Type: application/json' \
  --data-binary "@$ALIA_COMPANION_TMP/update.json" \
  "$ALIA_COMPANION_URL/api/v1/companion/policy")
printf 'HTTP %s\n' "$ALIA_COMPANION_STATUS"
jq . "$ALIA_COMPANION_TMP/update-result.json"
```

curl 非零退出、连接中断或客户端超时也不能证明 Server 已回滚；没有有效 JSON 响应时不要依赖最后一条 jq 的输出。保留 `update.json`，按以下分支处理：

- `200`：保存返回的政策与新 revision。再次提交同一旧 revision 会返回 `409`，不会多更新一次。
- `409 VERSION_CONFLICT`：先 GET 当前政策，核对其他更改。它不会自动把原意图应用到新 revision；确实需要再次修改时，应显式创建新的意图。
- `503 COMPANION_POLICY_UNCONFIRMED`，或传输完成结果未知：先重新 GET。不要立即换 revision，也不要认定写入失败。
- `400/401/403/500`：按下表检查输入、当前账户或服务状态，保留原请求供核对；不要把这些响应当作成功回执。

重新读取：

```sh
curl --fail --silent --show-error --max-time 10 \
  -H "Authorization: Bearer $ALIA_COMPANION_TOKEN" \
  "$ALIA_COMPANION_URL/api/v1/companion/policy" \
  > "$ALIA_COMPANION_TMP/observed.json"

jq -n --slurpfile intent "$ALIA_COMPANION_TMP/update.json" \
  --slurpfile observed "$ALIA_COMPANION_TMP/observed.json" \
  '{original_revision: $intent[0].expected_revision,
    current_revision: $observed[0].revision,
    policy_matches_original_intent: ($observed[0].policy == $intent[0].policy)}'
```

当前政策已符合原目标时，保留读取结果，无需再次写入；这只证明当前状态，不证明上一份 HTTP 回执已送达。若 revision 仍等于原 `expected_revision` 且目标尚未生效，可重发完全相同的 `update.json`；如原请求仍在完成，CAS 会避免双写。若 revision 已变化且政策不符合原目标，停止自动重试，由操作者核对后决定新的更改。GET 本身失败时，继续保留原意图，待读取恢复。

## shadow 原身份重试与回放

`ALIA_COMPANION_APPRAISAL_ID` 必须来自当前 owner 已有的规范 Appraisal 回执；不能用随机 UUID、其他人的 ID 或客户端自行推断的证据代替。下面新建一次操作；后续重试使用保存的同一个请求文件。

```sh
: "${ALIA_COMPANION_APPRAISAL_ID:?请提供自己的已有规范 Appraisal ID}"
ALIA_COMPANION_OPERATION_ID=$(uuidgen)
jq -n --arg operation "$ALIA_COMPANION_OPERATION_ID" \
  --arg appraisal "$ALIA_COMPANION_APPRAISAL_ID" \
  '{operation_id: $operation, appraisal_ids: [$appraisal]}' \
  > "$ALIA_COMPANION_TMP/shadow.json"

curl --silent --show-error --max-time 10 \
  -o "$ALIA_COMPANION_TMP/shadow-result.json" -w 'HTTP %{http_code}\n' \
  -H "Authorization: Bearer $ALIA_COMPANION_TOKEN" \
  -H 'Content-Type: application/json' \
  --data-binary "@$ALIA_COMPANION_TMP/shadow.json" \
  "$ALIA_COMPANION_URL/api/v1/companion/shadow"
jq . "$ALIA_COMPANION_TMP/shadow-result.json"

# 回放只读原操作；从保存的请求恢复身份，不生成新 UUID。
ALIA_COMPANION_OPERATION_ID=$(jq -r .operation_id "$ALIA_COMPANION_TMP/shadow.json")
curl --silent --show-error --max-time 10 \
  -o "$ALIA_COMPANION_TMP/replay-result.json" -w 'HTTP %{http_code}\n' \
  -H "Authorization: Bearer $ALIA_COMPANION_TOKEN" \
  "$ALIA_COMPANION_URL/api/v1/companion/replay/$ALIA_COMPANION_OPERATION_ID"
jq . "$ALIA_COMPANION_TMP/replay-result.json"
```

POST 返回 `503 COMPANION_SHADOW_UNCONFIRMED` 或传输结果未知时，只重发原 `shadow.json`；不能生成新 operation、增加 Appraisal 或延长期限。GET replay 返回 `503 SERVICE_UNAVAILABLE` 时重试相同读取。GET 的 `403` 同时覆盖不存在、其他 owner 和当前权限不足，不能用它证明前一个 POST 未提交。

政策版本变化、隐私/关系清除、来源撤销或到期可能使原私密证据不可再读；原 owner 在仍具读取权限时得到 `410 COMPANION_EVIDENCE_ERASED`。此时只能保留不透明操作身份，不能要求从旧回执、hash 或日志重建已擦除证据，也不能把 shadow 回执升级为 live 输出。

## 错误形状

JSON 错误统一外层为 `{"error": {...}}`，内部 `code` 和 `message` 必填。普通错误使用共享 `ErrorResponse`，认证错误另带机器可读的 `reason`；没有细分原因时省略该字段。客户端按 `code` 和 `reason` 判断，不匹配消息文字。Companion 的三个特殊回执使用独立类型，以下身份字段必须存在。

| 状态 | `error.code` | 附加字段与处理 |
| --- | --- | --- |
| 400 | `VALIDATION_ERROR` | 无；缺字段、未知字段、nil UUID、重复 ID、越界或 enabled 尚不可用 |
| 401 | `AUTH_INVALID_CREDENTIALS` | `reason` 为 `token_missing`、`token_invalid` 或 `account_inactive`；缺少/无效/过期 JWT 或账户不存在/非 active |
| 403 | `AUTH_FORBIDDEN` | 无；owner、来源或当前权限不允许；replay 不区分不存在与其他 owner |
| 409 | `VERSION_CONFLICT` | 无；CAS revision、operation 意图或规范权限版本冲突 |
| 410 | `COMPANION_EVIDENCE_ERASED` | 原 `operation_id`；shadow/replay 的私密证据已不能回放 |
| 503 | `COMPANION_POLICY_UNCONFIRMED` | 原 `expected_revision`；PUT 完成未确认，先 GET 再处理原意图 |
| 503 | `COMPANION_SHADOW_UNCONFIRMED` | 原 `operation_id`；POST 完成未确认，重试原 ID 和原 Appraisal 集合 |
| 503 | `SERVICE_UNAVAILABLE` | 所有接口的账户验证超时带 `reason: account_verification_unavailable`；policy/replay 的读取或存储不可用不带 `reason` |
| 500 | `INTERNAL_DB_ERROR` | 无；认证时账户查询的数据库错误，仅返回通用消息 |

PUT/POST 的 503 也可能来自进入业务操作前的账户验证超时，此时没有 Companion 回执身份字段；OpenAPI 同时声明共享错误与特殊回执。

replay URL 的 UUID 文本解析失败是现有 Axum 提取器返回的 `400 text/plain`；格式正确但为零的 UUID 则是 `400 application/json`。客户端应先检查状态和 Content-Type。应用内 Companion 请求预算为 2 秒；认证查询也有独立的 2 秒预算，客户端 deadline 不是服务端提交或回滚的证明。

## 验证与后续任务

- [x] 四个现有 REST 方法、完整 DTO、Bearer owner 权限和错误变体进入 OpenAPI/Scalar。
- [x] 终端步骤保留原政策 revision、原 shadow operation 与 worklist，说明 409/410/503 和传输未知边界。
- [x] 新增 `openapi::tests::companion_contract_matches_owner_routes_required_fields_and_real_errors`，序列化 ApiDoc 并对照实际响应构造器；不依赖 `/scalar` 的 200 来代替契约检查。
- [x] ApiDoc 契约回归已在 V4 的 macOS workspace 验证中通过；证据见 [API handoff](../refactor/handoffs/2026-09-28-r04/api.md)。
- [x] V4 的七项既有 Companion HTTP/生命周期回归通过，包含真实 PostgreSQL、owner/CAS/擦除/原身份重试与非法 UUID 的纯文本错误。本文 curl 操作示例尚未实际执行。

本页补齐现有服务契约和操作说明，没有新增 CLI、前端、依赖或自动启用政策。配置、启动、账号与设备管理入口见 [Server 与 Embedded 无前端操作入口](server-embedded.md)。Web/iOS/Android、Pi 真机安装与配对、声学验收及 24 小时运行验证仍按 [后续轮次](../refactor/checklists/next-round.md) 和 [延期台账](../refactor/deferred.md) 继续。
