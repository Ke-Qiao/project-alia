# 配置发布与恢复 Root HANDOFF

承接 `7165ad6`，分支 `codex/architecture-refactor`。继续三个 subagent：`skills_registry` 负责配置域持久化与内部故障测试，`skills_http` 负责 Setup 完成 owner、HTTP/CLI 接线与实际 drain，`skills_regression` 负责真实 HTTP 与丢弃旧 owner 后 loader 恢复。Root 负责 discovery/load/needs_setup、交叉审阅、唯一 Cargo/PG/Docker 窗口和正常提交。三方已交接。V2 Linux 发现旧播放测试缺少本地 gate 完成见证，V3 仅修该测试等待条件，保留原 3 秒整体期限与生产逻辑；首次失败和同二进制复查均已存档。

本轮 **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**，不是项目整体百分比。先检查旧实现再补缺口：私有候选、原子发布和单赢家沿用；发布/恢复归配置域，删除 OOBE 旧写入口。完整行为见 [技术说明](../../config-durability-2026-10-11.md)。

源码 `095499676496fba79853532b2ad4f570b368d37c`，V3 manifest `11373f598d31148c27b15bfa1fc5bb015e3b2726f877347560b23bbcc1b9343f`，650/650 Git blob 一致。

macOS workspace **1108/0/5**；Linux ARM64 Server 默认并行完整回归 **826/0/5**。两端无默认 feature 全目标检查、架构边界正反自测、变更文件格式检查、计数器 8 例自测通过。数字为 passed/failed/ignored；嵌套子进程摘要不重复累计，最终阶段同源、无源码漂移。

全部 Cargo 阶段结束；关闭前其他数据库连接为零，最终测试 schema 为零，遗留 schema 清理数 0。本轮 V1/V2/V3 Linux holder 与独立 PostgreSQL 已停止；用户 5433 未访问。空闲 holder 的停止码不作为 Cargo 测试结果。缓存、只读快照保留在 ignored `run/validation-20261011-config-durability/`。

## 恢复步骤

1. 读取 [检查点](../../resume-checkpoint-config-durability-2026-10-11.md)、本轮 checklist、其他三份 HANDOFF 和 [证据索引](evidence-index.json)，核对 branch/status/log。
2. 本轮无待恢复的实现或验证作业。新变更新建输入清单，不覆盖本轮原始日志与冻结记录。
3. 下一软件候选：先审计 `SettingsRegistry::save` 与 `PATCH /api/v1/settings` 的 live apply、内存配置及 `settings.toml` 持久化顺序，定义保存失败与重试的恢复语义，再在授权范围补缺口。保持 API、当前政策和权限；先检查既有实现，不直接扩展本轮发布协议为通用事务框架。
4. 可信文件系统部署边界、V01、R02/R03、Pi/24h/R05、R06 和前端等暂缓项目继续见 [延期台账](../../deferred.md)，不自行扩大硬件/平台/外部服务范围。

未推送、未建 PR，本轮未使用 reset 卡。

未运行真实断电、独立进程 kill/restart、Linux 全 workspace、release/native release link、部署 smoke、真实 PTY、外部或付费模型、Pi/声学/执行器及长稳验证。故障注入与丢弃旧 setup owner 后加载证明软件恢复语义；不代表任意存储设备兑现 flush，也不提供其他本机进程的文件系统锁。
