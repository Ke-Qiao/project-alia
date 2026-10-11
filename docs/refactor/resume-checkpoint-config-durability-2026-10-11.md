# 当前检查点：2026-10-11 配置持久性与发布恢复完成

后续运行时 Settings 轮已完成，最新恢复从 [Settings 检查点](resume-checkpoint-settings-recovery-2026-10-11.md) 开始；下文保留配置轮历史状态。
分支 `codex/architecture-refactor`；本轮 **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**，不是项目整体百分比。三个 subagent 已交接，无待恢复 Cargo/PG 作业。

依次读取 [技术说明](config-durability-2026-10-11.md)、[checklist](checklists/config-durability-2026-10-11.md)、[四份 HANDOFF](handoffs/2026-10-11-config-durability/README.md)、[证据索引](handoffs/2026-10-11-config-durability/evidence-index.json)，然后核对 branch/status/log。

源码 `095499676496fba79853532b2ad4f570b368d37c`，V3 manifest `11373f598d31148c27b15bfa1fc5bb015e3b2726f877347560b23bbcc1b9343f`，650/650 Git blob 一致。

发布/恢复归配置域；候选与 fragments、目录树和完整 canonical 祖先链同步；私有相邻 descriptor 记录同一意图。Setup 单一 watch 为 Idle/Pending/Ready/Cancelled，Pending 保留文件，仅同路径/内容/安装方式可恢复，取消封门但不删除记录。主文件未发布时启动也识别 pending；正常无记录 load 仍只读。保留 API、当前政策、来源限制和权限，不增加依赖或平台。

macOS workspace **1108/0/5**；Linux ARM64 Server 默认并行完整回归 **826/0/5**。两端无默认 feature 全目标检查、架构边界正反自测、变更文件格式检查、计数器 8 例自测通过。数字为 passed/failed/ignored；嵌套子进程摘要不重复累计，最终阶段同源、无源码漂移。

V1 库回归通过后补上失败遗留目录的完整父链同步，形成 V2。V2 Linux 首次在旧 Playback 测试的立即本地 gate 断言失败，同二进制默认 9 项复查通过；V3 仅把该测试的数据库回执与本地 gate 完成放到共同的原 3 秒期限内等待。生产 Playback 未改，首次失败保留；本检查点的最终数字全部来自 V3。

全部 Cargo 阶段结束；关闭前其他数据库连接为零，最终测试 schema 为零，遗留 schema 清理数 0。本轮 V1/V2/V3 Linux holder 与独立 PostgreSQL 已停止；用户 5433 未访问。空闲 holder 的停止码不作为 Cargo 测试结果。缓存、只读快照保留在 ignored `run/validation-20261011-config-durability/`。

**下一软件候选：先审计 `SettingsRegistry::save` 与 `PATCH /api/v1/settings` 的 live apply、内存配置及 `settings.toml` 持久化顺序，定义保存失败与重试的恢复语义，再在授权范围补缺口。保持 API、当前政策和权限；先检查既有实现，不直接扩展本轮发布协议为通用事务框架。**

可信文件系统服务账户与路径部署、真实断电、V01、R02 AEC、R03 反馈、Pi/24h/R05、R06 和前端等暂缓项仍开放，见 [台账](deferred.md)。自定义 setup 目标重启须继续指定相同 `ALIA_CONFIG_PATH`；恢复检测到改动/丢失时保留现场并报错，不能仅删除记录强行继续。

未推送、未建 PR，本轮未使用 reset 卡。未运行真实断电、独立进程 kill/restart、Linux 全 workspace、release/native release link、部署 smoke、真实 PTY、外部或付费模型、Pi/声学/执行器及长稳验证。故障注入与丢弃旧 setup owner 后加载证明软件恢复语义；不代表任意存储设备兑现 flush，也不提供其他本机进程的文件系统锁。
