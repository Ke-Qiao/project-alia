# Classifier 目录衔接 Root HANDOFF

承接 `4e4d595`。三个 subagent：`skills_http` 负责 Router，`skills_registry` 负责目录与版本一致性，`skills_regression` 负责真实 HTTP/EventBus/Interaction 回归；Root 负责 generation/prompt/cache 接线、审核元数据、架构守卫、唯一 Cargo/PG/Docker 窗口与提交。

实现说明见 [技术说明](../../classifier-2026-10-11.md)。核心闭环为单次目录快照 → 实际候选 → 名称校验 → 选中定义核对 → 当前模式回退；整目录指纹同时限制 exact/semantic 消费与待审核写入。工具可见性、Execution 批准和默认 Shadow 保持，没有新增依赖/feature/平台/模型/硬件动作。

V1 静态审阅发现类型导入路径、三处目录锁取消遗漏和 major-only 夹具替身接线，全部在编译前修正。V2 增加真实 COMMIT 写锁下 finish 取消回归，无测试生产钩子。目录变化发生在准入之后不会自动撤销已开始的回答；绑定原指纹的缓存不能在新目录命中。外部 SQL/多实例仍需显式 reload。

本轮 **10/10 完成**。

源码 `744cae2a11f8192f0e8ccf3d13004402202c1411`，V2 manifest `adeeaa0d150e6c7a036beeb05aa409da1d811e130d60354fe46e290c2ec132b5`，644/644 Git blob 一致。

macOS workspace **1080/0/5**；Linux ARM64 Server 默认并行完整回归 **798/0/5**。两端无默认 feature 全目标检查、边界正反自测和变更文件格式检查全部通过。计数依次为 passed/failed/ignored，嵌套子进程摘要不重复累计；所有最终阶段同源、无源码漂移。

所有 Cargo 阶段结束；关闭前其他数据库连接与测试 schema 均为零，任务 Linux holder 与 PostgreSQL 已停止，用户 5433 未访问。空闲 holder 的 sleep 进程在 docker stop 后 exit 137，验证命令均 exit 0；未把容器停止码当作测试结果。缓存与只读快照保留在 ignored `run/validation-20261011-classifier/`。

下一软件轮从已记录的 R04 整体 health 限额审计开始：核对 `api/health.rs`、ModuleRegistry 顺序聚合、Router 的 minor probe 和 Provider 诊断，先查已有实现，再收敛整体预算、重复探测与并发边界。默认 probe=false，保持 API 和当前政策；未在本轮实施，不自行启用网络付费探测。

原 V01、R02/R03/R05/R06、前端和未来设备侧部署继续见 [延期台账](../../deferred.md)。L09—L12 保持 4/4，六轮仍 2/6，不代表项目整体百分比。继续三个互斥 subagent，Root 统一冻结验证和正常提交。

## 恢复步骤

1. 读取 [当前检查点](../../resume-checkpoint-classifier-2026-10-11.md)、checklist 与其余三份 HANDOFF，核对 branch/status/log 和证据索引。
2. 本轮没有待恢复作业或未完成实现。V1 为静态候选，V2 为最终首次同源验收；新变更另冻清单，不能覆盖原证据。
3. 按上述 health 审计继续，先给出具体范围与步骤，保持三个互斥 subagent 和 Root 唯一 Cargo/PG 验证窗口。
4. 原 V01、R02/R03/R05/R06 与 D/H 门槛不变；不访问用户数据库或自行发起硬件动作。

未推送/未建 PR；本轮未使用 reset 卡。

未运行 Linux 全 workspace、release/native release link、真实 PTY、部署 smoke、Skills 网络 COMMIT ACK 丢失注入、真实模型或硬件验证。目录写锁取消注入覆盖 finish 阶段；exact/semantic 目录等待未分别重复同样锁注入，已审阅包装并运行既有缓存取消出口回归。多实例／外部 SQL 仍需显式 reload。
