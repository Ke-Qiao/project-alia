# 原输出的持久控制账本 — 2026-09-22

账本工作树从 `76f5b04` 继续，原基线为 `8c3f545`。SQL 组件已完成真实
PostgreSQL 定向验收；完整 Server/Embedded 可恢复播放仍由纵向整合包验收。
文件名保留旧 WIP 路径，便于此前检查点和交接记录继续定位。
详细条目与证据见 [账本 checklist](checklists/duplex-ledger.md)。

## 持久契约

新增迁移 `000013` 和 `delivery::repository::control`，复用
`DeliveryRepository`。规范 Response 已 prepare 后，才可登记唯一 voice scope、
原 Context、Server instance 和 native backend；登记允许早于首次 AudioDelivery。
另一个 Response 占用同 scope 返回 typed `Conflict`，不能改签原绑定。

登记截止时间起始为 Response 创建后 120 秒与已有 audio observation deadline 的
较早值。新控制每次取得 Delivery 锁后还会读取后加入音频的真实观察期限，并在同一
事务中将 scope 截止时间仅向小收紧；Resume 释放 hold 后该硬期限仍保留。
Pause/Confirm/Resume 的许可不能超过该期限，重试不延长原登记或原 hold。
单个 hold 的 60 秒上限不缩短无额外音频期限的 120 秒 scope 上限。
仍为 pending/sending/observing 的音频 owner 必须维持有效租约；其过期后拒绝新控制。
已终结的 Submitted/Completed 前缀不要求续租，迟到规范媒体事实仍能记录。
默认 hold 为 60 秒、硬上限 120 秒；这些数值仍是工程默认，并非用户确认的产品值。
运行时必须遵守 `ControlPermit.deadline_at()`，不能用较晚的 scope 登记上限替代它。

所有新控制走原 Context 完整门，锁顺序为 policy → active account → privacy →
relationship → canonical evidence → Response → control → Delivery。原政策、来源、
Memory revision/private/expiry、关系来源根或 cancel/owner 失效均不能换用新 Context
继续。只有首次事务明确提交成功才返回不可 Clone 的 `ControlPermit`。
`Existing`、查询和 `CommitUnknown` 均只允许核对事实，不授权重发命令。
网络工作不在 SQL 事务中执行。

两张表每 scope 最多 32 条命令。每 command 保留 first/latest 回执，加 scope latest，
最多 65 份。首次 outcome 固定，与设备当前 snapshot 分开；当前 snapshot 只接收更高
observation sequence，且 command watermark 不得超过 SQL 中实际已接纳序号。
迟到的 Unknown 命令仍可保存原 outcome，但不能覆盖更新的当前 snapshot。
已淘汰的旧观察返回 `Stale`，不伪称已证明相等；普通 Observe 不追加审计事件。

取消、永久退休、设备晚事实不依赖旧 Context 仍有效。恢复仅退休真实已过期的 scope，
不能把“非本 instance”视作死亡，也不接管旧 PCM。本地 Retired 不因迟到 Running
事实复活。控制回执里的 media progress 只保存原事实，不改写规范 Delivery 进度、
assistant history 或音频 dispatch。

## 验证边界

2026-09-22 原 9 项首跑为 7 通过、2 失败：关系用例缺少真实 Appraisal 来源根，反向
policy 用例在制造竞争前错误地调用需取更新锁的 `current()`。两处夹具修正后原 9 项
通过。新增的观察期限、watermark 和 scope 冲突 3 项均先在旧生产代码失败，再随
对应生产修复通过。随后实际复现并修正了 active audio owner 租约过期仍可 Resume
的问题；最终目标为 13 通过、0 失败、0 ignored，测试体 3.94 秒。

验证包括真实 COMMIT ACK 丢失、原 hold/容量上限、审计失败整事务回滚，以及实际
`AliaMemory.clear`、`ContextService.clear_relationship`、政策 CAS 和
`DeliveryRepository.request_cancel` 与 Resume 的双事务锁屏障。每例随机 schema，
隔离清理；不操作用户 5433、历史 schema 或其他容器。

独立编译使用先前保留的 Core-only 临时覆盖，共 6 个路径；覆盖清单与哈希单独保存，
不混入账本提交。完整新版 Proto 与运行时 owner 尚待主代理组合验证，不能把这里的
SQL/静态检查当作两端端到端播放验收。日志与失败候选保存在工作树
`run/validation-output-control-20260922/`。
