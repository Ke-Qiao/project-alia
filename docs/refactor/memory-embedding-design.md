# Memory 向量投影的身份与重建

更新：2026-09-14。关联：[Context 契约](context-revocation-design.md)、[构建记录](progress.md)。

Memory 内容由规范 `memory_items` 管理。向量是可重建投影，其存在不赋予来源权限，也不能改变规范内容。当前实现移除固定 1536 维表、人工 profile 别名和 Memory 对 Cache 开关的依赖。

## 实际请求身份

`EmbeddingProfile` 描述实际使用的 provider、endpoint、model、dimensions、distance 和 vector type，其摘要成为持久政策中的 profile ID。当前 revision 表示 `request-config.v1` 契约，不能证明远程服务在相同模型名下保持同一权重。凭证不属于向量空间身份。Provider 必须提供可核验的请求预览，并通过 `embed_profiled` 返回实际描述符和逐输入摘要；旧式 `embed()` 不能为 Memory 投影提供来源证明。

新安装由配置预览播种持久 profile。已有数据库的政策胜过进程启动配置，重启不能静默更换向量空间。管理员明确应用模型、地址或维度配置时，完整描述符登记与持久政策 CAS 在同一事务提交，再发布运行时设置；Memory 和 Cache 各自保存 pin。直接修改 `core.policies.memory.active_embedding_profile` 被拒绝，该值仅供查看。未配置 embedding 时为 NULL。

`embedding_profiles` 保存完整不可变描述符。Memory 和 Cache 复用描述符校验函数，但分别拥有投影表、工作来源和授权用途。`memory_embeddings` 按 `(memory_id, profile_id)` 唯一，保存规范 revision、精确输入摘要、原始 policy version、privacy epoch、维度和向量；数据库验证 `vector_dims` 与记录一致。不同维度先经 materialized CTE 隔离，再进行距离计算。

## 写入与迟到结果

1. 规范写入在同一事务内固定 `MemoryEmbeddingWork`：owner、Memory ID、新 revision、输入文本和摘要、当时的政策/隐私版本、预期实际描述符。
2. 发请求前，在有期限的短事务中验证当前政策、账户、epoch、规范 revision、状态、过期和敏感限制，再比较 adapter 预览。事务结束后才调用 provider。
3. 返回结果必须匹配固定描述符、单输入摘要、数量和有限数值。提交事务重新验证原始身份；不能读取新政策给旧工作重新签名。
4. 同一 revision、输入与 profile 的已有投影保留，重复完成不会覆盖。规范更新在原事务内删除旧向量；新旧推理乱序完成时，旧 revision 无法写回。删除或清除后也不能重建已撤销的记录。

更新 Memory 会使读取其旧 revision 的生成 Context 失效。这不意味着新规范 revision 永远不能建立自己的索引；投影以成功提交的新规范版本为工作根，不再次借用已失效的旧生成 Context。

初次 Context 物化在查询 embedding 之前核验持久原文、owner、当前 epoch；扩展 Context 还核验原 lease 的全部根。请求结束后的快照事务再次完整验证。独立 Memory 检索也在请求前后检查政策与账户版本，不能用新的 pin 解释旧查询向量。

## 有界后台重建

一个受运行时托管的 worker 直接扫描规范记录，不另建无来源的任务副本。它按 `(owner, memory_id)` keyset 每批读取 16 个候选，成功或失败均推进游标，到末尾后 30 秒再扫。坏记录不会阻塞后续候选；中断的 inline 写入、进程重启或明确 profile 切换留下的缺失投影可在后续扫描恢复。

Inline 投影与 worker 共用两个推理 permit；查询向量属于调用方预算，不使用这两个投影 permit。Inline 投影预算 2 秒，后台请求预算 30 秒，数据库阶段预算 2 秒；没有在数据库锁内等待模型。关闭先取消网络/SQL future，再等待 worker 和所有 permit 释放。规范 Memory 写入成功与投影成功分别记录；模型不可用时仍可管理记忆并走关键词检索。

敏感、`private_runtime`、失效或过期内容不参与外部 embedding。当前检索优先使用实际 profile 一致的投影，无法取得合法向量候选时按现有有界关键词路径退化。该实现没有宣称大数据量 ANN 性能、真实远程 provider 延迟或 Pi 端推理已经验证。

## 迁移与验证

`m20260913_000009_memory_embedding_identity.sql` 删除旧 `memory_embeddings_1536` 并创建新的投影表；不为旧向量编造请求身份。规范 Memory 内容保留，新表由合法当前来源重建。用户已确认无历史数据迁移要求；已提交 SQL 不改写。

真实 PostgreSQL/pgvector 用例覆盖多维度、同维度跨模型、Cache 关闭、错误输入摘要/实际 profile、旧 revision 晚返回、clear、政策切换、请求失败恢复、后台清除竞争及 shutdown join。配置 HTTP 用例验证只读 pin、显式切换、重启持久身份及部分发布重试。最终计数与平台范围以构建记录为准，定向重跑不重复累计。

Appraisal/Relationship 的派生根与规范 Memory 递归祖先校验正在单独的构建包中接入。已提交的子 Memory 在父记录改版、私有或过期后如何退出 Context、独立检索和投影，仍需该包的实际闭包验证；本包的隐私 epoch 与自身 revision 校验不能替代它。
