# Root HANDOFF — 2026-10-14

基线 `66ae27ec1f19a69bbac2765938fdb18efdd35deb`，分支 `codex/architecture-refactor`。审计限于现有 Settings 路径和共享 privacy gate；生产代码未改，新增两项真实 Registry future 取消回归。当前源码已冻结，统一验证窗口由 Root 持有。

三位 subagent 沿用前轮：settings_auth_fix 做 await/锁/预算与取消审计，settings_auth_reproduce 补离线回归，settings_auth_review 核对 R02/R03/R05 和硬件资料。后续不从本次风险清单自动衍生全面 Provider 重构。

- [x] 基线、范围、分工和先查已有实现。
- [x] Settings 静态审计及硬件/部署资料清单。
- [x] 集中向用户询问 H01—H05，实际值未收到时保持未知。
- [x] 当前窄验证与独立审查收口。
- [x] 证据、恢复入口、正常提交和干净工作区核对。

已修正当前 `local-development.md` 中过期的 health 诊断说明，与已实现的 6 秒/8 并发/重复探测去重规则一致。新增 `hardware-deployment-readiness.md` 集中列出历史事实、确认缺口、只读采集命令、R02/R03 首包和 R05 的实际出口；命令只是准备，没有 SSH、录放音、串口或机构动作。

本轮验证只选择改动相关的 Settings integration、原存储回归以及进入 R02/R03 前的 Embedded 软件基线；不为两项测试重跑无关 Server 全量/DB。此前双平台通过记录保持为历史，不冒充本轮 Linux 或实机通过。

## Root 最终收口

本轮审计与准备 checklist **8/8**。测试源码 `cffbca531cec2478c17b5bcacfd191e7f00a56f9`；冻结输入 **656/656** 与 Git blobs 一致，manifest `bb52f12217b548c4d152992101dd304448a0581fccb690c1967b97d9967b819c`。生产源码、API、政策、权限与依赖未改。

当前 macOS ARM64：Settings 原回归 **5/0/0**、新取消回归 **2/0/0**、存储回归 **7/0/0**、Embedded 默认 lib 软件基线 **177/0/0**。默认测试并行度，原期限保持，所有阶段输入无漂移；格式、架构边界正反自测和差异检查通过。原始命令、日志与哈希见 [证据索引](evidence-index.json)。

所有验证进程已退出；未创建/启动外部数据库、容器或常驻服务，未运行 SSH/录放音/串口/机构动作。Embedded 软件 fixture 可使用临时 SQLite 与回环 socket；stage JSON 的 database=not used 专指外部/PostgreSQL 数据库。

H01—H05 仍待当前部署确认，R02-A/R03-A 为下一可执行软件包，尚未实施；六轮仍 **2/6**，L09—L12 **4/4**。本轮不宣称当前 Linux/workspace/release、真实模型、AEC、控制板反馈或24小时整机验收通过。客户端断开/真实handler取消、慢模型与共享gate传导、同步文件阻塞仍待独立动态验证；R05前须关闭对应恢复门槛。
