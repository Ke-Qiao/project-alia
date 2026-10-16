# 当前 release 全 noop staging 生命周期与恢复验收

2026-10-17；基线 `1c63218`，源码 **`6c62c84`**，分支 `codex/architecture-refactor`。本轮 **10/10 完成**。

- [x] 核对源码状态、上一轮release身份及既有staging/运行证据。
- [x] 三个subagent审计模板、生命周期与独立证据，明确本轮契约。
- [x] 准备任务隔离配置、PG、internal network和运行目录，固定源码/native/二进制身份。
- [x] 有界脚本保留原readiness/请求/关闭期限和默认测试并发。
- [x] 保存V1实际失败与确定性RED，确认sensor loop取消域缺口后单点修复并GREEN。
- [x] 新源码Linux完整workspace、两端no-default和default release/native链接通过。
- [x] V2两轮真实启动、SIGTERM关闭、同配置/DB/身份/desired settings恢复通过；权限与shadow保持。
- [x] 每轮端口、进程、PG回收核对，所有任务容器/卷/network精确删除，共享服务保留。
- [x] 四份HANDOFF、全局台账、恢复检查点与148份证据归档收口，保留原失败/派生计数边界。
- [x] 正常提交受测源码，678项Git blob与冻结输入匹配；文档与证据交接正常提交。

Linux一次1287/0/5/0filtered；Mac定向2/0不与整包相加。原Cargo标题分块、原INVALID计数保留，有界派生视图经独立复核；不修改原日志或重复测试求绿。完整结果见[Root HANDOFF](../handoffs/2026-10-17-release-lifecycle/root-HANDOFF.md)。

保留默认shadow、当前政策/权限、精确取消及清理保证，R03暂停；无新依赖、平台或真实硬件。恢复指正常关闭后的重启，不等同断电/强杀、真实模型或物理状态恢复。历史V01根因仍open；六轮仍2/6。
