# Memory 修复与验证记录

更新：2026-09-08。本包建立规范记忆的所有者、版本、来源和遗忘边界，并接通对话及工具入口。默认特性 workspace 在专用空 PostgreSQL 数据库上 500 项通过、0 失败；其中 memory_privacy 14 项通过（13 PG + 1 RAM），既有 memory 13 项通过。完整命令与范围见[构建记录](progress.md)。这不代表 Companion、真实 Embedding 质量或所有遗忘传播已经完成。

## 规范数据与遗忘

| 问题 | 当前行为 | 验证 |
| --- | --- | --- |
| 删除后图、向量或派生内容仍存在 | 同事务推进用户 privacy_epoch/cutoff，递归擦除派生内容、向量、图与来源关系；墓碑清空正文、摘要、证据和元数据。 | PG 删除源及两级派生、图/ContextPack 查询、事务错误回滚。 |
| 晚到抽取或投影重新写入旧内容 | 写入时在用户锁内核对 epoch、原输入时间、源消息归属与源记忆 revision；向量返回后重验规范记录。 | 跨实例旧快照、空 clear、旧来源晚落库、源修改/过期、删除与创建竞争、晚到向量。 |
| 模型指定其他用户的记录 | 规范读写、显式 ID 和依赖都检查 owner；修改需要 expected revision，后台只采纳本次实际读取的 ID。 | PG 跨用户更新、删除、ID 覆盖、记忆与消息来源被拒绝。 |
| 数据库失败变成 RAM 成功 | 配置 PostgreSQL 后失败明确返回；RAM 仅为显式测试后端。 | 真实连接池关闭、删除/clear/topic 触发器失败及原子回滚。 |
| 不同语言或数值被误合并 | 匹配只规范大小写、已知用户前缀和空白，保留 Unicode、否定及标点。 | PG 自动去重与确定性整理覆盖 1.5/15、can't/cant、can/can't、中文肯定/否定及准确重复。 |

确定性 AliaMemory::consolidate_user 先形成纯变更计划，再在一个原始快照及事务中提交。自身的第一次更新不会使同批后续更新失效。投影使用提交后的规范 revision，不使用已经被本批改变的旧 revision。

## 可信输入贯穿调用链

CLI 在读取输入行、进入身份校验前固定 Server 时间；legacy 音频在 RPC 入口固定；连续语音保存该话段首个仍保留音频帧的 Server 接收时间，包含有限预录音。裁剪、排队、模型重试和继续说话不会刷新原时间。不采用设备时间。Core 13 项和真实 gRPC 入口 6 项回归覆盖这条契约。

process_text 在历史读取后、LLM/工具执行前提交 append_sourced_message。数据库在同一事务验证 session owner、插入用户消息并更新会话，返回实际消息 UUID。当前轮次后续持久化复用该 UUID，避免重复用户消息。用户消息提交失败会阻止模型/工具处理；助手回复保存失败会停止 cache、turn memory capture 和 ConversationTurnCompleted。普通消息插入和会话更新也已改为事务。真实 PG 触发器及 Pipeline 故障测试验证这些行为。

ToolContext → Execution 意图 → worker 持久传递来源时间及可选消息 ID。相同幂等请求保留首次接收时间；消息 ID 是意图身份的一部分。AI MemoryStore 必须有已提交的真实用户消息，写入时验证归属、时间与当前隐私边界。REST/调度保留各自可信请求或触发时间。MemoryStore 的新增与更新都使用 guarded API，不能在分派时刷新来源。25 项 Execution service 测试包含清除后排队 AI 记忆工具被拒绝、消息缺失/跨用户/时间不符，以及合法来源成功写入。

turn capture 在 LLM 之前读取隐私快照，并始终使用原输入的消息 ID/时间。Insights 只从拥有会话的同一规范 UUID 用户读取来源明确的 user 行；没有来源时间的历史、助手行、其他用户消息或 cutoff 之前的来源不会参与后台抽取。

## 当前限制与下一主线

- LLM 多项整理仍逐条提交。首次修改可能使同一快照的后续操作失效；现在明确报错并说明已提交数量，禁止吞错或使用模型 summary 冒充全部完成。两个调用链回归验证不刷新快照、不继续后续创建。原子 LLM 变更批次仍需实现，与已经通过的确定性整理不是同一能力。
- MemoryForget 工具的多项删除仍需一个明确的批次及原请求来源校验，当前不能宣称旧删除请求与新记忆竞争已受完整保护。
- 响应缓存、已在途 ContextPack、待投递内容、Companion 与暂停设置的完整遗忘传播仍属于后续主线；不放入用户指定的延期台账。
- 删除使用保守的用户级 cutoff，可能同时拒绝该用户其他在途旧来源的抽取；之后的新输入仍可记忆。

## 验证入口

参照构建记录的 workspace 命令，或分别运行 cargo test -p alia-server --test memory --test memory_privacy --locked，并设置指向可丢弃 PostgreSQL 的 ALIA_TEST_DATABASE_URL。

每个专项 PG 用例使用随机 schema，真实执行完整内置迁移及清理。缺少环境或连接失败明确失败，不静默跳过。pgvector 由启动准备安装到共享 public schema，测试 schema 删除不拥有该扩展。没有使用用户的 alia-postgres 数据库。受控 Embedding 替身只用于制造晚返回，数据库和投影提交均真实执行。最新 Memory 隐私测试耗时 1.02 秒，既有 Memory 测试 0.20 秒；它们不证明真实模型质量或硬件性能。
