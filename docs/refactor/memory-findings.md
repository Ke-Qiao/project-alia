# Memory 修复与验证记录

更新：2026-09-08。规范记忆拥有 owner、revision、来源和隐私边界，对话、后台整理和工具均使用这些边界。本轮完成 LLM 原子变更批次、批量遗忘和可信来源 epoch；最终 workspace 561 项通过、0 失败，其中 memory_privacy 24 项、既有 memory 13 项全部通过。完整整合命令与最终结果见[构建记录](progress.md)。这不代表 Companion、真实 Embedding 质量或所有遗忘传播已经完成。

## 规范数据与遗忘

| 问题 | 当前行为 | 验证 |
| --- | --- | --- |
| 删除后图、向量或派生内容仍存在 | 同事务推进用户 privacy_epoch/cutoff，递归擦除派生内容、向量、图与来源关系；墓碑清空正文、摘要、证据和元数据。 | PG 删除源及两级派生、图/ContextPack 查询、事务错误回滚。 |
| 晚到抽取或投影重新写入旧内容 | 可信接纳时固定 source epoch，写入时在用户锁内核对当前版本、源消息持久版本与归属、源记忆 revision；向量返回后重验规范记录。 | 跨实例旧快照、空 clear、旧来源晚落库、源修改/过期、删除与创建竞争、晚到向量。 |
| 模型指定其他用户的记录 | 规范读写、显式 ID 和依赖都检查 owner；修改需要 expected revision，后台只采纳本次实际读取的 ID。 | PG 跨用户更新、删除、ID 覆盖、记忆与消息来源被拒绝。 |
| 数据库失败变成 RAM 成功 | 配置 PostgreSQL 后失败明确返回；RAM 仅为显式测试后端。 | 真实连接池关闭、删除/clear/topic 触发器失败及原子回滚。 |
| 不同语言或数值被误合并 | 匹配只规范大小写、已知用户前缀和空白，保留 Unicode、否定及标点。 | PG 自动去重与确定性整理覆盖 1.5/15、can't/cant、can/can't、中文肯定/否定及准确重复。 |

确定性 AliaMemory::consolidate_user 先形成纯变更计划，再在一个原始快照及事务中提交。自身的第一次更新不会使同批后续更新失效。投影使用提交后的规范 revision，不使用已经被本批改变的旧 revision。

## LLM 批次、退役与批量遗忘

`MemoryBatch` 在一次原始读取快照上提交全部更新和创建。PG 在同一用户锁和事务内验证全部 ID、owner、源 revision、来源消息和 privacy epoch，再提交规范记录与图投影；RAM 测试后端在副本上验证完成后整体替换。未知或重复目标、revision 冲突、创建 ID 冲突和后续 SQL 故障都不能留下前半批变更。向量更新在规范提交后进行，返回时重新检查规范状态，不能让晚到向量复活被遗忘内容。

LLM consolidation 先完整解析和验证计划，再调用一次 `apply_batch_guarded`。删除指令已从模型协议移除，改为带理由的 `superseded` / `rejected` 退役；原文与证据仍保留，后续用户遗忘可以递归删除它和派生内容。修改保留原有规范来源并合并新来源，模型不能通过替换 metadata 切断真实证据。所有阶段的候选抽取也统一一次批量提交；最终数量来自数据库回执，模型 summary 不作为提交证明。

MemoryForget 只接收明确 ID 或查询中的一种，先固定隐私快照和可信请求来源，再读取目标及 revision，最后调用一次 `forget_many_guarded`。多项遗忘在同一事务内推进一次 privacy epoch；查询不会在提交阶段重新检索并误删后来出现的内容。AI 工具必须携带已持久化的源消息。退役记录仍能成为显式遗忘目标，空目标不推进 epoch。

真实测试覆盖多项更新与创建共同提交、后续变更失败整体回滚、完整来源保留、批量遗忘和写入竞争、退役后递归遗忘，以及旧请求不能删除清除后新建的无关记忆。提交 ACK 丢失可能导致调用方无法确认，因此错误只报告“批次未确认”，不保证每种连接错误都意味着数据库从未提交。

## 可信输入贯穿调用链

原始 Server 接收时间继续贯穿排队与处理，用于审计，不采用设备时间。隐私因果顺序改由可信入口在数据库中接纳时固定的 `source_privacy_epoch` 证明。CLI、两种音频入口、REST 和调度的具体接纳位置见 [来源契约](source-admission.md)。持续语音不能通过续期给旧窗口改签新版本；版本变化会退休旧会话。

process_text 在历史读取后、LLM/工具执行前提交 append_sourced_message。数据库在同一事务验证 session owner、插入用户消息并更新会话，返回实际消息 UUID。当前轮次后续持久化复用该 UUID，避免重复用户消息。用户消息提交失败会阻止模型/工具处理；助手回复保存失败会停止 cache、turn memory capture 和 ConversationTurnCompleted。普通消息插入和会话更新也已改为事务。真实 PG 触发器及 Pipeline 故障测试验证这些行为。

ToolContext → Execution 意图 → worker 持久传递来源 epoch、时间和可选消息 ID。相同幂等请求保留首次接纳数据；epoch 与消息 ID 是意图身份的一部分。AI MemoryStore 必须有已提交的真实用户消息，验证当前版本、持久来源版本、归属和同一消息的审计时间。MemoryStore / Forget 的读取前验证和最终 guarded commit 分开执行，去重也不能绕过来源验证。

turn capture 在 LLM 之前检查原输入 epoch，并始终使用原消息 ID / 时间；晚返回再次通过 guarded commit 验证。Insights 只读取同一规范 UUID 用户、所属会话中的 user 行，且来源持久 epoch 必须匹配当前 snapshot。NULL stamp、助手行和其他用户来源都不参与后台抽取，不再拿 Server 时间和数据库 cutoff 比较来证明先后。

Insights 在取得并发许可后重新查询账户是否 active，缓存的近期用户列表不构成授权。手动 trigger 和 schedule 均返回真实阶段错误；失败日志不能把失败变成成功。日志写入失败明确说明此前阶段变更可能已经提交，完成事件只在日志成功后发布。pending 计数使用 RAII，在取消、等待许可或提前返回时都能释放。5 项真实 PG/Runtime 调用链测试验证旧快照、日志 INSERT 故障、缓存用户停用、排队重查及快慢钟来源；模型仅作为受控故障替身。

## 当前限制与下一主线

- 响应缓存、已在途 ContextPack、待投递内容、Companion 与暂停设置的完整遗忘传播仍属于后续主线；不放入用户指定的延期台账。
- Memory 批次与 Insight 日志/Cache 并非一个跨模块事务。日志失败会明确返回不确定的阶段结果，不能抹去此前已提交的 Memory 事实。
- 删除推进保守的用户级 epoch，可能同时拒绝该用户其他在途旧来源的抽取；之后新接纳的输入仍可记忆。cutoff 仅保留数据库审计意义。

## 验证入口

参照构建记录的 workspace 命令，或分别运行 cargo test -p alia-server --test memory --test memory_privacy --locked，并设置指向可丢弃 PostgreSQL 的 ALIA_TEST_DATABASE_URL。

每个专项 PG 用例使用随机 schema，真实执行完整内置迁移及清理。缺少环境或连接失败明确失败，不静默跳过。pgvector 由启动准备安装到共享 public schema，测试 schema 删除不拥有该扩展。没有使用用户的 alia-postgres 数据库。受控 Embedding 替身只用于制造晚返回，数据库和投影提交均真实执行。最终 workspace 中 Memory 隐私测试 24 项耗时 3.26 秒，既有 Memory 13 项耗时 0.20 秒；这些结果不证明真实模型质量或硬件性能。
