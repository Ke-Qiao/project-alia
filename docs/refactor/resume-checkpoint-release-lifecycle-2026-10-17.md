# 恢复检查点：当前 release 全 noop staging 生命周期

2026-10-17；分支 `codex/architecture-refactor`，基线 `1c63218`，源码提交 **`6c62c84ee60e3a5fd387f2172ead432031907599`**。本轮 **10/10 完成**，四份交接与证据另随正常文档提交保存。先读[checklist](checklists/release-lifecycle-2026-10-17.md)与[四份HANDOFF](handoffs/2026-10-17-release-lifecycle/README.md)。

## 已完成与恢复依据

- V1旧release正常SIGTERM发生Server exit1，确定为sensor内层discovery取消与外层owner取消的窗口；先保存RED1/1，再只改为background.token()。GREEN2/0/0，真实提前退出保护保留，原资源均join。
- 修订源码Linux默认完整workspace一次 **1287/0/5/0filtered**，67 Running+5 Doc，405.963秒；两端no-default与default release/native link通过。未改变原测试期限/并发，5项既有ignored保持。原日志一处Cargo标题分块，原INVALID计数保留，仅派生视图与明确行映射用于确认计数，无测试重跑。
- 新release两轮实际启动、正常SIGTERM关闭、同状态重启全部通过：两端4个进程exit0，无兜底/强杀，三端口关闭；同账号、device/key/config、shadow、ETag恢复。纯非政策temperature=0.25仅证明desired settings保存/重载，noop回执明确ignored_runtime；settings同hash且0600，SQLite同路径/schema与integrity正常，不声称字节相同。
- 678输入manifest `0133b6f493b3c8b39b365abe03e3cfc035d88b0f21962e0ed6abd745796ed8c7`，对应源码Git blob全部匹配。Server SHA `0374aa62602a3e6c2908459b7e8a4a574bd937071d182dbb18172eccb0695616`；Embedded SHA `159becdbd3783b77ad308d02096ed39158fc54cfef43f648091a2c0f015f4003`。独立copy位于ignored `run/validation-20261017-release-lifecycle/v2/artifacts/`。
- V1与V2任务PG均0/0后精确删除容器/卷/internal network，共享服务保留。没有运行中的Cargo或本轮服务。私有runtime保留用于核对，但任务PG卷已删除，不能只重启旧容器复用为新的验证环境。
- [148份归档证据](handoffs/2026-10-17-release-lifecycle/evidence-index.json)共1541367字节，保留原环境预检失败、V1失败、RED/GREEN和V2成功。密钥/密码/完整settings不归档；脚本依赖其原任务目录，不是独立部署框架。

## 下一步 checklist（不属于本轮完成项）

- [ ] 汇总当前软件发布资料，先核对最新Git/源码/产物身份；明确全noop通过与真实模型/部署未验收的边界，不盲目重复已通过的同源批次。
- [ ] 集中确认模型服务的实际端点、版本与运行方式，以及Mac/Pi服务账户和独立部署目录；不接收密码/密钥，未明确前不启用外部服务。
- [ ] 在用户确认范围内完成真实模型链的软件验收，保留默认shadow和现有权限/执行批准、失败与清理证据。
- [ ] 若有新的V01失败或可区分条件，继续有界定位；本轮sensor shutdown竞态不能倒推解释历史Companion配对超时。
- [ ] 现场资料与授权齐备后再进入R02声学/AEC和R05整机验收；R03在用户恢复前继续暂停。

仅既有Debian12/Linux ARM64全noop正常重启通过；未验证crash/断电、Mac/Pi独立部署、真实录放音/模型、执行器、其他平台或长稳。R02板/固件、功放/扬声器与AEC电气资料仍缺，其他延期见[台账](deferred.md)。六轮仍 **2/6**，不是项目完成百分比。
