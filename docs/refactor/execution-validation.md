# Execution 验证记录

更新：2026-09-08。整合后的默认 workspace 在专用空 PostgreSQL 上 500 项通过、0 失败。Execution repository 19 项通过（2.04 秒），Execution service 25 项通过（20.36 秒），均无忽略。状态、API 和故障语义分别见[Execution 设计](execution-design.md)和[操作契约](execution-api.md)。

## 真实数据库与故障边界

| 层次 | 已取得的证据 |
| --- | --- |
| Repository 幂等 | 并发同键只有一个 effect；不同用户分离；不同参数或来源消息 ID 拒绝冲突；provider call ID/接收观测时间变化的重试保留原始事实。 |
| 批准与认领 | 批准和 Outbox、认领和 Outbox 消费均同事务；真实触发器故障无部分状态；并发取消与认领只有一个获胜。 |
| 当前权限与版本 | 用户停用、角色下降、catalog/实现/策略变化、批准过期均不能使用旧授权认领。 |
| 结果与恢复 | 结果、审计原子提交；认领标识校验；相同回执幂等、冲突回执拒绝；过期 dispatched 恢复 unknown，绝不自动重放副作用。 |
| 资源 | 每用户 32 个 proposed、总队列 256、4 个 worker；并发不能突破额度；有界分派和结果持久化。 |
| 提交确认不确定 | 真实 PG COMMIT 已执行后截断、扣住 ACK 或 drop 提交等待者，取消所有权仍保留，旧会话意图不能因丢失 ACK 而脱离管理。 |
| 关闭与等待 | 持续表锁验证提交 5 秒、整批关闭 10 秒预算；超时列出未完成 ID 并保留 JoinHandle。锁住账本也不能让工具结果等待无限延长或伪造取消。 |
| 固定 executor | 同 schema 热替换会拒绝未认领旧版本；已认领任务使用原 Arc，不被新注册替换。 |
| 可信记忆来源 | 请求入队、重试和 worker 保留时间/消息 ID；AI MemoryStore 缺消息、跨用户、时间不符、clear 后旧请求被拒；合法来源可写入。 |

适配器探针只用来控制调用时点、返回和副作用计数。PostgreSQL、事务锁、故障触发器及 COMMIT ACK 代理均真实运行，不用 mock DB 替代。source 时间的重试断言和最新的工具结果等待期限测试已包含在以上数量中。

## 复现

参照[构建记录](progress.md)的 workspace 命令，或运行 cargo test -p alia-server --test execution_repository --test execution_service --locked，并设置 ALIA_TEST_DATABASE_URL。

环境为 macOS 26.6.2 ARM64、Rust 1.95.0、pgvector/pg17 的专用临时容器。用例随机创建 schema，运行内置迁移，结束时清理；缺失环境变量或数据库不可达明确失败。完整回归没有操作用户 alia-postgres 或真实硬件执行器。

首次从完全空库运行 workspace 暴露 pgvector 被安装进临时 schema 的问题。现在数据库准备在同一个数据库级锁下固定安装到 public；第二个无扩展、无表的空库已通过完整回归。旧迁移内容未改写。

## 尚未由本包证明

Execution 账本证明意图、授权、分派和适配器回执，不证明声学呈现或机构到位。整体 Server 在 Execution 返回未完成时保留出站通道和数据库、启动失败回滚及长期连接退出，仍需[Server 生命周期包](server-runtime-design.md)实现；service 级保留句柄不能替代宿主生命周期。多租户框架、真实设备配对、真实外部副作用核对和持续运行验收也不在这些测试的声明范围内。
