# 当前检查点：2026-10-10 L12 完成

本检查点为历史记录；Skills HTTP 后续清理已完成，最新从 [2026-10-11 检查点](resume-checkpoint-skills-2026-10-11.md) 继续。

分支 `codex/architecture-refactor`；本轮 **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**，不是整个项目完成百分比。三个 subagent 已交接，任务容器/数据库全部停止，无待恢复 Cargo/PG 作业。

依次读取 [技术说明](layering-l12-2026-10-10.md)、[checklist](checklists/l12-2026-10-10.md)、[四份 HANDOFF](handoffs/2026-10-10-l12/README.md) 和 [证据索引](handoffs/2026-10-10-l12/evidence-index.json)，核对 Git 状态后继续。

Delivery 现在拥有出站 transport、payload/observer/report 和共享音频预算；Channel 保留入口职责，原 adapter 与 producer 绑定不被 registry 替换。旧 Channel/TTS 契约入口删除，无兼容层。默认 Shadow、当前政策、两类暂停、原子性、精确取消、限额、native owned cleanup 保持。

源码 `d9f97216a6599742c45e0a6389110b6b465e8461`，V4 manifest `bc542c0718b07fc45c1f7e09084a51fddfa3350014eee089514e6a582f9e75ea`，641/641 Git blob 一致。

macOS workspace **1060/0/5**、真实 PTY **3/0/0**、无默认 availability **2/0/0**、提前准入 **1/0/0**；Linux ARM64 Server 默认并行完整测试 **778/0/5**。两端无默认全目标检查及 Linux release/native link 均通过。计数依次为 passed/failed/ignored，子进程摘要不重复计入。 所有最终结果同源，无源码漂移。

Linux 当前主要启动瓶颈已测量：实际 sha2 对同一 462699808 字节文件 15.416→0.834 秒，摘要完全一致；只调整已有 sha2 的 dev 编译优化。最终 V4 默认并行 child 2.983 秒，其中 build 1.833 秒、native release 2ms、schema DROP 87ms。 保留 20 秒期限和默认并发。L11 历史失败没有阶段数据，不能宣称所有历史原因被证明；原 V01 仍独立开放。

V3 macOS setup 旧端口新连接断言失败，最终 V4 改为原 listener drop 见证和原连接 EOF/reset，有界关闭、取消拒写和原响应要求全部保留。失败原时刻的 peer 未知，端口重用仅候选；复查通过与最终修复证据分开归档。

**下一项软件任务：核对 `packages/alia-server/src/api/skills.rs` 中创建/更新/删除后的三个 reload TODO 与 `modules/interaction/skills` 的实际索引更新路径；先检查既有实现，再补缺口和真实整合回归。保持工具可见性、批准、加载/卸载/热更新和当前政策；本轮未实施该任务。**

继续三个互斥 subagent，Root 统一执行验证和正常提交。后续仍需处理原 V01、R02 AEC、R03 物理反馈、Pi/24h/R05、其他平台/R06；前端/未来设备端部署等暂缓项见 [延期台账](deferred.md)。不扩依赖/feature/平台/模型/硬件动作，不访问用户 5433。

资源关闭前 schema/其他连接均为零；任务缓存保留于 ignored run。未推送/未建 PR，用户允许额度不足使用 reset 卡，本轮未使用。

未运行 Linux 全 workspace 或部署 smoke，未验证真实模型、Pi/声学/机械及其他平台。L11 历史超时没有阶段日志，不能追认其全部原因；V3 setup 失败当时连接到谁仍未证实。
