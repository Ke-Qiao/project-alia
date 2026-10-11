# Root — Settings 等待后身份重验证

最终检查项 **8/8 完成**；以下过程说明保留各实施时点，最终结果以文末 Root 验收记录为准。
基线 `bdc94dd245251a45b620877be728a7635d3e1296`，分支 `codex/architecture-refactor`。本轮按用户要求先审计与真实复现，再补纯非政策 PATCH 的最终准入缺口；没有扩大权限、政策、依赖或平台。

三个 subagent：`settings_auth_fix` 修改唯一生产文件 `api/settings.rs`；`settings_auth_reproduce` 负责 HTTP/PG 回归；`settings_auth_review` 只读独立审查。Root 负责集成、配置说明、源冻结、统一运行、资源关闭和提交。开始恢复时已有本轮 RED 未提交测试与日志；核对输入后接续，没有覆盖已有工作。

## 实施 checklist

- [x] 核对原 AuthUser、更新 gate、SQL current、preview 和政策 actor/CAS。
- [x] 保留首次屏障失败，核对第二次有效 RED 的 654 项输入与未变生产源。
- [x] 最终非政策分支 2 秒查询 active admin；403/固定 503；无通用授权框架。
- [x] 8 项真实 HTTP/PG 定向 V1 通过；独立审查并修复仅测试失败清理。
- [x] 冻结 V2：654 输入，`1f88c037cc584c41b88efba5141858930926b1c066f2b0b881e7579e490a90e1`。
- [x] macOS workspace/Server no-default、Linux ARM64 Server/no-default 同源验收。
- [x] 资源关闭、源码 blob 核对与证据归档。
- [x] checklist、四份交接、恢复检查点和正常提交。

## 行为与证据边界

RED V2 政策行锁等待后已提交降权，原实现仍返回 200，desired/live 8→9、文件改变、模块应用一次。修复 V1 定向 8/0/0；原用例现 403，副作用为零；lookup timeout 的真实锁等待约 2003 ms 后 503。V2 仅避免请求任务 panic 时二次 poll 已完成 JoinHandle，生产逻辑相同。最终完整运行另行记录，不能用定向计数冒充全量。

API 与默认政策、SQL actor/CAS、部分成功、显式重试、首次 setup 恢复均不变。核验只读取 id；没有跨 module apply 的用户行锁。SQL current 可以独立执行暂停到期转换，因此拒绝保障针对本请求设置副作用。最终准入之后才提交的降权不追溯撤销操作。

Linux 首次准备因缺少空 source 目录失败，发生在复制和启动容器之前；已补目录后从同一 V2 冻结输入准备成功。不是 Linux Cargo 或生产源码失败，单独保留准备记录。

纯更新 gate 排队和慢 JSON body 未单独动态复现；有静态时序以及政策/preview 等待见证。GET Settings 的等待后身份核验为下一候选。本轮未扩大为全系统撤权审计，未运行真实 PTY/Easy-Turn、Linux 全 workspace、release/部署、Pi/声学/执行器或长稳。

## Root 最终验收补记

源码 `4553e88138cb91af46f024b343baaa3857c8d769`；最终 V2 manifest `1f88c037cc584c41b88efba5141858930926b1c066f2b0b881e7579e490a90e1`，654/654 源码 Git blobs 一致。

macOS ARM64 workspace **1128/0/5**；Linux ARM64 Server 默认完整回归 **846/0/5**。两端 Server 无默认 feature 全目标检查、架构边界正反自测、变更 Rust 格式、差异和计数器 8 例自测通过。数字为 passed/failed/ignored，嵌套子进程摘要不重复计入。全部最终阶段同源且无漂移，保持默认测试并发和原期限。新增 8 项身份回归均在两端全量中通过，V1 定向 8/0/0 不额外加到全量数字。

RED 首次屏障失败与 V2 有效越权复现分别保留；GREEN V1 定向通过后，仅修复测试 panic 清理再冻结 V2。Linux 首次准备缺少空目录，复制源码和启动容器前失败；补目录后准备成功，未变更生产源码。最终 Cargo 阶段均通过。

全部 Cargo 已结束，本轮 PostgreSQL 和 Linux holder 已停止。关闭前其他 DB 连接 0、测试 schema 0，遗留 schema 清理 0 个；用户 5433 未访问。原日志、源码快照与缓存留在 ignored `run/validation-20261012-settings-authorization/`，可核对副本及哈希见证据索引。空闲 holder 停止码不是 Cargo 结果。

本轮总 checklist **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**，不是项目完成百分比。前文记录实施时点，已由本节关闭最终验证、资源、源码提交待项。

未独立动态验证纯 update gate 排队或慢 JSON body 窗口；最终核验位置由静态顺序与真实 policy/preview 等待验证。未运行真实 PTY/Easy-Turn、Linux 全 workspace、release/部署 smoke、外部付费服务、Pi/声学/执行器或长稳。最终核验是一次准入，准入后的降权不追溯撤销已开始的更新。
