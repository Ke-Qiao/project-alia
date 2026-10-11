# Settings 等待后的身份重验证

本轮承接 `bdc94dd`，清理纯非政策 `PATCH /api/v1/settings` 的身份快照过期窗口。先审计、复现，再修复；不扩大到其他入口、持续撤销、跨模块事务或新依赖。进度见 [checklist](checklists/settings-authorization-2026-10-12.md)。

入口认证从 SQL 读取有效用户并生成 `AuthUser`。但取得更新 gate、读取政策、两次 embedding preview 都可能等待。原非政策分支直接沿用入口角色，等待期间管理员已降权仍可更新 desired、模块、live 和文件。政策及 embedding 分支已有 SQL 事务内 active admin 核验与 CAS，保留其原实现。

修复只在非政策分支最终准入点查询当前 active admin 的 id，查询等待有 2 秒上限。已降权、停用或删除返回原 403；数据库错误或超时返回固定脱敏 503。核验通过前不产生本请求的设置副作用。SQL current 的自动暂停到期转换是既有独立行为，不能声称任何拒绝都绝无 SQL 变化。

准入后的部分成功与显式重试规则不变；不会在 module apply 期间长期锁定用户行，也不追溯撤销已经准入的操作。每次显式重试重新认证和准入。API、默认 shadow、工具可见性、执行批准、精确取消、资源所有者及依赖均保持。

## 复现证据

保留生产未修改时的两份冻结清单和日志。RED 首次仅测试屏障失败，不能作为业务缺陷证据。V2 将客户端构造移到有界见证前，并补只读诊断；真实政策行锁与 settings gate 已被观察，随后提交管理员降权。原实现仍返回 200，desired/live 从 8 变为 9，文件改变，模块应用 1 次。V2 的 654 个输入核对一致；TLS 初始化影响首次见证仅为推测，未确认根因。

## 本轮交付与后续

Root 统一运行验证、管理任务数据库和 Linux 容器；三个 subagent 分别负责生产修复、真实 HTTP 回归、独立审查。最终运行结果、源码身份、资源关闭与提交写入 [四份 HANDOFF](handoffs/2026-10-12-settings-authorization/README.md)。

下一候选是 `GET /api/v1/settings` 在等待后返回含敏感配置的设置前的身份有效性审计，先复现再决定修复；本轮不修改读取 API。V01、AEC、执行器反馈、Pi/长稳、其他平台和前端仍见 [台账](deferred.md)，不能由本轮软件测试关闭。

## 最终验收

源码 `4553e88138cb91af46f024b343baaa3857c8d769`；最终 V2 manifest `1f88c037cc584c41b88efba5141858930926b1c066f2b0b881e7579e490a90e1`，654/654 源码 Git blobs 一致。

macOS ARM64 workspace **1128/0/5**；Linux ARM64 Server 默认完整回归 **846/0/5**。两端 Server 无默认 feature 全目标检查、架构边界正反自测、变更 Rust 格式、差异和计数器 8 例自测通过。数字为 passed/failed/ignored，嵌套子进程摘要不重复计入。全部最终阶段同源且无漂移，保持默认测试并发和原期限。新增 8 项身份回归均在两端全量中通过，V1 定向 8/0/0 不额外加到全量数字。

RED 首次屏障失败与 V2 有效越权复现分别保留；GREEN V1 定向通过后，仅修复测试 panic 清理再冻结 V2。Linux 首次准备缺少空目录，复制源码和启动容器前失败；补目录后准备成功，未变更生产源码。最终 Cargo 阶段均通过。

全部 Cargo 已结束，本轮 PostgreSQL 和 Linux holder 已停止。关闭前其他 DB 连接 0、测试 schema 0，遗留 schema 清理 0 个；用户 5433 未访问。原日志、源码快照与缓存留在 ignored `run/validation-20261012-settings-authorization/`，可核对副本及哈希见证据索引。空闲 holder 停止码不是 Cargo 结果。

未独立动态验证纯 update gate 排队或慢 JSON body 窗口；最终核验位置由静态顺序与真实 policy/preview 等待验证。未运行真实 PTY/Easy-Turn、Linux 全 workspace、release/部署 smoke、外部付费服务、Pi/声学/执行器或长稳。最终核验是一次准入，准入后的降权不追溯撤销已开始的更新。
