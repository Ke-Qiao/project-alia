# 关系状态、Appraisal 与重建

本页描述 Server 的 REST 操作。所有关系接口都按当前数据库中的有效账户授权，只操作已认证用户自己的状态；客户端不能提交 owner ID。部署级解释政策由管理员通过 Settings 管理。Web、iOS、Android 的界面仍在后续工作台账中。

## 查询和清除

| 请求 | 请求体 | 结果 |
| --- | --- | --- |
| `GET /api/v1/expression/relationship` | 无 | 当前有效证据重建的关系摘要、隐私设置和 `relationship_epoch` |
| `DELETE /api/v1/expression/relationship` | `operation_id`、`expected_epoch` | 持久清除回执，包含原版本、新版本和实际移除数量 |
| `POST /api/v1/expression/relationship/prune` | `limit`，范围 1–256 | 清理当前用户已过期的投影与判断，返回实际移除数量 |
| `POST /api/v1/expression/relationship/rebuild` | `operation_id`、`after`、`limit`，范围 1–32 | 一页显式重建的来源数量、提交回执、跳过/待完成数量及下一页游标 |

例如先查询当前版本，再发清除请求：

```json
{"operation_id":"4dd11d43-a464-421e-a3a3-39211392186d","expected_epoch":0}
```

操作 ID 必须由调用方生成且非空，不能复制上述示例作为所有请求的固定 ID。同一 ID 和原版本的重试返回原回执；改变该 ID 对应的参数会返回 `409`。使用新 ID 携带过期版本也返回 `409`，应重新查询当前状态后明确决定是否发起新的清除。

清除关系推进独立的 `relationship_epoch`，移除本域 Appraisal、Affect 投影和由其派生的 Memory。独立的普通 Memory 保留，隐私版本不因关系清除而变化。已接纳输入保存原来的两个版本，清除前排队的输入不能通过晚到、重试或模型执行重新取得关系权限。Memory 清除另外推进隐私版本，并撤销依赖它的 Appraisal。

清除接口有 2 秒请求预算。数据库错误、数据库等待超时或 HTTP 等待超时都以 `RELATIONSHIP_CLEAR_UNCONFIRMED` 的 `503` 响应保留原 `operation_id` 和 `expected_epoch`，只能以同一意图重试核对。超时或连接失败本身不能证明事务已提交或已回滚；明确的权限和版本冲突仍返回对应的 `403`/`409`，不泄露内部 SQL 错误。

## 显式重建

政策变化不会悄悄重新解释历史输入。需要恢复关系投影时，由用户显式申请一页重建：

```json
{"operation_id":"69f7feb2-cdd1-43f3-84fc-02552d92a84a","after":null,"limit":16}
```

Server 在解释之前持久化这一页的 owner、政策版本、两个来源版本、游标、数量上限和固定来源清单。只有已有规范 Appraisal 证明过的用户原文可成为候选；重建不自动重放旧 Memory recall、助手响应或工具完成判断，也不为无来源的旧记录补造权限。

每个来源有独立的领取 token，遗留领取在 15 秒后可被接管。最终判断与重建项回执在同一事务提交，迟到的旧 token 不能写入。原文通过 SQL 重新读取并校验，持久任务不复制原文。重建使用专门的 Context 用途，只允许解释和情感读取，不能直接授权生成响应、工具、缓存或 Memory 写入。新判断仍以原输入时间生效，并且有效期不能超过旧判断的过期上限。

完成的一页返回 `200`；若同一页已有其他请求在处理，返回 `202` 及 `pending` 数量。`503 RELATIONSHIP_REBUILD_UNCONFIRMED` 保留原操作 ID、游标和 limit，应以完全相同请求核对。重试不会纳入后来出现的新来源。只有完成当前页后才使用 `next_after` 和新的操作 ID 请求下一页；空页表示当时没有更多候选。

原操作的政策或来源版本变化后，继续重试会被拒绝。调用方必须重新查看政策和关系状态后，明确发起新的重建意图；不能自动把旧操作换成新的权限。后台 Appraisal writer 拥有已经接纳的写入，调用方取消等待不等于撤销已经发生的事务。

## 政策与边界

管理员的 `GET /api/v1/settings` 返回政策版本 ETag；PATCH 可用 `If-Match` 做版本比较。`core.policies.affect.*`、`core.policies.appraisal.interpretation_timeout_ms`、`core.policies.appraisal.approved_interpreters` 和 `core.resources.appraisal_lexicons` 属于持久政策。写入失败不发布进程内设置，重启时已存在的数据库政策覆盖配置 seed。内置 interpreter/reducer 的版本标识仅供读取。

`relationship_profile_user_visible` 控制用户摘要读取，`relationship_profile_user_clearable` 控制新清除操作。已提交清除回执仍可用原意图核对。账户停用后，即使 JWT 尚未过期也不能继续调用这些接口。

Appraisal 是可失效的解释，不是客观事实。当前响应相关判断依据规范 Response 提交；实际音频呈现、用户是否听到、身体机构是否到位，分别依赖 Delivery 与设备证据，不能从这里的摘要或情感值推断。
