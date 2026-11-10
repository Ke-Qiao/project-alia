# API 设计待考察事项

本轮只记录这些行为和影响，留待后续决定是否调整。未声明的接口另见 [OpenAPI 覆盖表](openapi-coverage.md)。

## 1. 登出只吊销提交的 refresh token

**现状。** [`refresh`](../src/api/auth.rs) 在同一个事务中吊销旧 token 并签发新的 token；[`logout`](../src/api/auth.rs) 只查找并吊销请求中那一条 token，旧 token 已吊销时也返回成功。[刷新 token 数据模型 `Model`](../src/entity/refresh_token.rs) 没有会话或轮换链标识。[JWT `Claims`](../src/auth/jwt.rs) 也没有会话标识；[`AuthUser::from_token`](../src/auth/middleware.rs) 验签并查询账户状态，不查登出记录。

**影响。** 刷新先提交时，随后用旧 token 登出仍返回成功，但新 refresh token 仍可用。账户保持启用时，已签发的 access token 也不会因登出而立即失效。Web 的 [`signOut`](../../../clients/web/lib/auth/sign-out.ts) 先清本地会话再尝试登出，[`createTokenRefresher`](../../../clients/web/lib/auth/token-refresh.ts) 拒绝将晚到的刷新结果写回已清除的会话。网络失败时也可能只完成本地退出。

**待考察。** 明确定义退出当前设备、退出全部设备的含义，是否要求同一会话轮换出的 token 一起失效，是否要求 access token 即时失效，以及刷新和登出并发时由哪方决定最终状态。

## 2. 请求提取失败的状态码和响应格式不一致

**现状。** 会话 [`get_messages`](../src/api/conversations.rs) 的 `Path` / `Query` 和 [`logout`](../src/api/auth.rs) 的 `Json` 由 Axum 直接提取。参数反序列化失败或 JSON 语法错误默认返回纯文本 400；请求体超过提取器限制返回 413；媒体类型缺失或不匹配返回 415；JSON 合法但字段缺失或类型不符返回 422。相同类型的 JSON 提取错误在 [Execution 的 `ExecutionApiError`](../src/api/effects.rs) 会变成域错误 400，在 [Companion 的 `update_policy`](../src/api/companion.rs) 会变成通用校验错误 400。

**影响。** 客户端不能总用 `error.code` 或 `reason` 区分失败；类似的输入问题在不同接口可能得到不同 HTTP 状态。[OpenAPI](../../../clients/web/openapi/openapi.json) 为登录、注册和 `PATCH /api/v1/users/me` 的 400 同时声明了 `application/json` 和 `text/plain`，但当前 `@hey-api/openapi-ts` [生成的 `LoginErrors`、`RegisterErrors` 和 `UpdateMeErrors`](../../../clients/web/lib/api/generated/types.gen.ts) 将 400 类型都写成 `ErrorResponse`。Web 的 `toApiError` 仍需接收实际出现的纯文本。

**待考察。** 先定哪些提取错误应保留各自的 HTTP 状态，再统一结构化错误映射，并为受影响的操作逐一声明响应。验证生成器如何处理同一状态的多种媒体类型，保留真实的 OpenAPI 声明。

## 3. 通用错误和域错误的 `code` 缺少共同分类

**现状。** [通用 `ErrorCode`](../src/error.rs) 用大写下划线，[`ExecutionErrorCode`](../src/api/effects.rs) 和 [`DeliveryErrorCode`](../src/api/deliveries.rs) 用小写下划线；[`CompanionErrorDetail`](../src/api/companion.rs) 回执用大写下划线，并要求携带原操作 ID 或原修订号。例如，通用权限错误用 `AUTH_FORBIDDEN`，Execution 和 Delivery 的部分 403 用 `unauthorized`。

**影响。** 跨模块客户端不能只靠统一字符串规则判断错误含义或重试动作。有些域错误携带恢复所需的身份信息，合并时不能丢掉这些字段。Web 的 [`toApiError`](../../../clients/web/lib/api-error.ts) 接受任意字符串，却在类型上将其当成通用枚举成员；域错误、新代码或代理响应里的未知字符串仍可能出现，调用方不能把枚举当作运行时保证。

**待考察。** 明确通用错误分类与域内恢复语义各自的职责，统一服务端与生成客户端的类型，明确如何表示未知代码。

## 4. 无权访问和不存在的会话可以被区分

**现状。** [`load_authorized_session`](../src/api/conversations.rs) 先按 UUID 查记录。查无记录返回 404；查到记录，但调用者既非创建者也非管理员，返回 403。详情、删除、消息、事件和表达跟踪共用这条路径。

**影响。** 知道或猜到 UUID 的已登录用户可以判断记录是否存在，无法凭此读取内容。这个差异是否可接受取决于会话存在性是否也算私有信息。

**待考察。** 决定是否只在有权查看记录的情况下区分这两个状态；如需隐藏存在性，再考虑按权限过滤查询，并同步调整声明与调用方。

## 5. 管理员能通过更新入口把自己的账号标记为已删除

**现状。** [`update_user_me`](../src/api/users.rs) 直接调用按 ID 更新的 `update_user`。普通用户不能改 `role` 或 `status`；管理员可以改自己的这两个字段，`validate_status` 允许 `deleted`。同文件的 `delete_user` 明确阻止删除自己。

**影响。** 两个入口对同一个软删除状态的限制不同。管理员可以通过更新让自己失去访问权限；若是最后一个管理员，且没有另外的恢复入口，也可能锁住管理权限。这是策略不一致，不是普通用户可以提权。

**待考察。** 决定自助资料能改哪些字段，管理员是否能自行降权、停用或删除，以及最后一个启用的管理员是否要有保护和恢复流程。

## 6. 会话历史会截断，没有下一页；跟踪顺序也不等同于时间顺序

**现状。** [会话查询参数 `ListQuery` 和 `TraceQuery`](../src/api/conversations.rs) 提供 `limit`，表达跟踪另有 `turn_id` 过滤。`list_conversations` 按更新时间倒序，最多 500 条；`get_messages` 和 `get_events` 按时间正序，最多 5000 条。`get_expression_traces` 先截取至多 5000 个源事件，[`expression_traces_from_events`](../src/expression/trace.rs) 再按 turn UUID 分组并排序。

**影响。** 达到上限后不能用 API 接着读取；较长会话的后续消息和事件可能一直读不到。跟踪的截断点可能落在一个 turn 中间，返回数组的顺序也不能当作时间线。

**待考察。** 明确各列表的读取方向和稳定排序键，为超长历史提供继续读取的游标。表达跟踪另需决定分页单位是完整 turn 还是源事件，以及如何标记不完整的 turn。
