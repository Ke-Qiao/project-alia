# /health 清理 Root HANDOFF

承接 `d27490f`。三个 subagent：`skills_registry` 负责 Registry 聚合与内部测试，`skills_http` 负责 Router/Gemini，`skills_regression` 负责真实 HTTP 回归。Root 负责 Builder profile 所有权、交叉整合、唯一 Cargo/PostgreSQL/Docker 验证窗口和正常提交。三个 subagent 已完成交接。

本轮 **10/10 完成**，L09—L12 保持 **4/4**，R01—R06 保持 **2/6**，这些数字不是项目整体完成百分比。实现见 [技术说明](../../health-2026-10-11.md)。

核心闭环：一轮 6 秒绝对期限／8 项并发 → Registry 单轮准入 → 实际实例去重映射回角色 → 保留完成结果 → 超时、drop、stop 回收 future。相同配置 profile 共用一个 lifecycle owner，custom override 独立；Router 不递归探测；Gemini 补独立 5 秒期限、配置快照与安全错误。API、默认 probe=false、Shadow、工具可见性与执行批准保持，无新增依赖、feature、平台或硬件动作。

V1 两端首次编译均因聚合 stream 内 async mapper 的高阶借用生命周期泛化失败而 exit 101，没有测试通过数字。确认两端退出后，V2 仅把 mapper 改为先构造借用 Future Vec 再进入 buffer_unordered，期限、并发、取消和测试逻辑均未改变；两个版本清单与原始日志全部保留。

源码 `29420a57d6558d90c36098c061655dff40eb72f4`，V2 manifest `9902508f7743bbbae5d447fec76c753d9f8910b0b1b019ef3ea5f801d03a21b0`，646/646 Git blob 一致。

macOS workspace **1094/0/5**；Linux ARM64 Server 默认并行完整回归 **812/0/5**。两端无默认 feature 全目标检查、边界正反自测、变更文件格式检查和计数器 8 例自测全部通过。数字依次为 passed/failed/ignored，嵌套子进程摘要不重复累计；所有最终阶段同源、无源码漂移。

所有 Cargo 阶段结束；关闭前其他数据库连接为零，最终测试 schema 为零（遗留 schema 清理数 0）。V1/V2 Linux holder 与任务 PostgreSQL 已停止，用户 5433 未访问。空闲 holder 的 sleep 容器停止码不作为测试结果；V2 验证命令均 exit 0，V1 编译失败 exit 101 另行保留。缓存与只读快照保留在 ignored `run/validation-20261011-health/`。

## 恢复步骤

1. 读取 [当前检查点](../../resume-checkpoint-health-2026-10-11.md)、checklist 与其余三份 HANDOFF，核对 branch/status/log 和 [证据索引](evidence-index.json)。
2. 本轮没有待恢复验证作业或未完成实现。V1 是失败编译输入，V2 是最终验收输入；新变更另冻清单，不能覆盖历史证据。
3. 下一软件任务：配置目录持久性。先核对 base 文件与 fragments 的创建、同步和原子发布调用链，定义发布后父目录 sync 失败的未知结果、目录树/祖先同步和恢复规则，再补必要实现与隔离文件系统回归。保持当前 API、政策、权限与已有依赖；真实断电不能用进程 kill 代替。本轮未实施这项。
4. 继续三个职责互斥的 subagent，Root 统一冻结与验收；原 V01、R02/R03/R05/R06 与 D/H 门槛见 [延期台账](../../deferred.md)，不扩大硬件或部署动作。

未推送／未建 PR，本轮未使用 reset 卡。

未运行 Linux 全 workspace、release/native release link、部署 smoke、真实 PTY、外部或付费模型、Pi/声学/执行器及长稳验证。6 秒预算约束合作式异步检查，不能强制中断同步阻塞；本地取消不代表远端立即停止处理请求。
