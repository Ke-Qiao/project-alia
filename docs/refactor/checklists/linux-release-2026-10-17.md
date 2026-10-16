# 当前版本 Linux 软件发布验收 checklist

2026-10-17；基线da3eca2/source4e2968f，分支codex/architecture-refactor；当前10/10完成。

- [x] 核对当前提交、工作区和历史Linux证据身份，区分未覆盖范围。
- [x] 三个subagent分别审计环境/native链接、测试矩阵和独立验收边界。
- [x] 冻结当前源码，核对私有缓存、固定镜像、native归档和隔离PG。
- [x] 在既有Linux ARM64环境执行一次默认完整workspace，原并发与期限保持。
- [x] Server与Embedded分别完成no-default all-targets检查。
- [x] 两端default release真实构建，核对新产物身份、ELF、动态库及native链接。
- [x] 仅对实证缺口修复并补有意义的回归，保留失败原件；无缺口则不改生产。
- [x] 独立核对结果、ignored、源码/产物身份及所有任务资源回收。
- [x] 更新四份HANDOFF、平台记录、全局台账和恢复检查点。
- [x] 正常提交并核对最终Git输入与受测快照一致。

默认shadow、当前政策、测试期限/并发不变；R03暂停。不新增依赖，不扩大到x86/Windows等平台；不启用真实音频、执行器或模型。此次不把build/link或help检查称为整机进程部署验收。

一次完整workspace1285/0/5/0filtered，两端no-default及default release/link通过。677输入与同源Mac及基线Git一致；50份原件已归档。数据库两次0/0，两个任务容器、卷及network已回收。未发现需修复的源码缺口，生产与测试不变。
