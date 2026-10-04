# 2026-10-04 · 证据核对、V01、Linux 与 R02/R03 前置

本包按用户指定的有界范围补缺口，已完成并正常本地提交。源码为 `010f7aa5ae2c39a5c90c7f22979391a6b1bbbf23`，633 项冻结输入与提交逐项一致。DU 11/11、R01 CP7/7、R04 原四组修复及 MG05 完成，六轮仍为 **2/6**；这不是项目完成百分比。本包软件通过不关闭 AEC、执行器或整机验收。

## 四份 HANDOFF

- [Root / runtime](runtime.md)：交付身份链、5 个 ignored、架构差距表、V01 与 setup/health 补充审查。
- [Linux](linux.md)：实际默认 feature、release 链接、二进制身份与无硬件 PG/HTTP/gRPC 生命周期。
- [Audio / R02](audio.md)：原生采集连续性修复、KWS reset、reference/时钟/测量出口与硬件信息表。
- [Motion / R03](motion.md)：公开可达与控制授权、原身份/boot 拒绝、mock 故障证据及物理反馈缺口。

## 本轮交付 checklist

- [x] 独立核对 R01/R04 Git 与 630/633 输入、旧证据 51/36 份哈希、原始日志和去重；未覆盖旧失败快照。
- [x] V01 有界调查：保留 V4 binary 三批 9/9；新增原期限/阶段安全诊断并复用确定性真实 PG 过期回归。**调查已做，V01 根因仍 open**。
- [x] 补 setup 单次完成、Host/Origin/JSON 与关闭期间拒写；核对 health 聚合和目录持久性，开放项明列。
- [x] R02 软件缺口修复与既有 reference fixture 复核；R03 新增两项实际 loopback 协议拒绝；未操作实物。
- [x] macOS 完整冻结回归、真实 PTY 与单次新诊断运行；原模型通过只作为历史证据。
- [x] Linux 默认 release/link、实际两轮启动/停止/重启与 8 个目标测试完成，实际 feature、动态依赖和产物身份已记录。
- [x] 源码正常提交并与 633 输入逐项核对；本轮两个容器已停止，四份 HANDOFF、证据索引和恢复 checklist 已归档。

## 当前验证口径

冻结输入 `a1c7dc3d6787cf2b86144b60c17b5aaaaad65025d1695a1326194c826c8ba0d0`，633 项。macOS workspace **1062/0/5**、541.76 秒，真实 PTY **3/0/0**，合计 **1065 个本轮不同 macOS 通过测试**；另一次配对 9/9 是重复验证诊断，不再累计。两个真实 Easy-Turn ignored 本轮未重跑。各已完成阶段 source drift 为空。

| 当前实测 | 结果 | 证据边界 |
| --- | --- | --- |
| macOS workspace + 真实 PTY | 1062/0/5 + 3/0/0；1065 个不同通过用例 | 两个真实模型 ignored 本轮未执行；重复诊断不增加数量 |
| Linux ARM64 默认 release | Server、Embedded 均构建/链接成功，动态库无缺失 | 当前容器及固定工具链；不等于所有 OS/CPU/features |
| Linux 实际进程 | 两轮 PG/HTTP/gRPC 启停和重启，4 个进程 SIGTERM exit 0，无残留 | noop 模型/音频、临时账户与设备状态；无硬件、模型推理或实际 Bind |
| Linux 必要测试 | 8 个目标，578/0/3，默认测试并行 | 3 个 PTY ignored 未执行；不与 macOS 或历史数字相加 |
| V01 有界复测 | 旧保留 V4 binary 三批 9/9，新诊断一批 9/9 | 原 V3 failing artifact 未核实保留；未复现，根因仍 open |

最终命令、退出码、输入和二进制身份收录于 [验收摘要](validation-summary.json)、[Linux 摘要](linux-summary.json) 和 [提交输入核对](committed-source-verification.json)。原始日志保留 `run/validation-20261004/`，[证据索引](runtime-evidence-index.json) 记录原件及归档副本的哈希，保留旧失败快照。633 是输入目录清单规模，不是整个运行环境保证。

三个 subagent 已完成，没有活动 Cargo/PG 窗口。本轮新 DB/Linux 容器均已停止，任务非 public schema 和其它连接为 0；旧任务 DB 保持 stopped，用户 5433 容器保持原 running 状态，未查询或修改用户数据库。模型服务未探测，不能由本报告断言其当前健康；详见 [停止后资源状态](resource-cleanup.json)。没有推送或创建 PR。

下一次从 [当前检查点](../../resume-checkpoint-2026-10-04.md) 和 [机器状态](../../resume-state-2026-10-04.json) 继续。H02—H04 当前接入/设备/机构信息、V01、R04 health/目录持久性、R02/R03/R05 实机出口仍开放；前端、通用 Embedded、其它平台和端侧 Easy-Turn 不扩张。
