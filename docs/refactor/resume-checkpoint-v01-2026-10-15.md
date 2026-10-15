# 当前恢复检查点：V01 旧 Companion 配对超时有界调查

日期2026-10-15；分支`codex/architecture-refactor`；基线`c685e5a`，测试诊断提交`f81762395de0ad75efa9c1e7afe60f46b620cac1`。本调查包**10/10完成**，三个subagent flow7/7、fixture6/6、review6/6均收口；**V01历史根因仍open**。R01—R06仍2/6，R03继续暂停。

先读[checklist](checklists/v01-2026-10-15.md)、[四份HANDOFF](handoffs/2026-10-15-v01/README.md)与[Root结论](handoffs/2026-10-15-v01/root-HANDOFF.md)，核对Git状态再接续。已执行三批上限，不重复相同测试求绿。

## 已完成与限制

- 历史15份证据身份核对通过；原V3失败binary在10个有界候选中未找到，不能用现存V4/current代替原失败现场。
- baseline固定binary两批默认并行各9/0/0（14.40/13.28秒）；诊断版完整workspace一次1245/0/5，436.72秒，其中duplex10/0/0、13.77秒。5项为原PTY3/Easy-Turn2未运行，无新增ignore或filter。当前运行与旧环境不同，未复现不能证明修复。
- 唯一代码差异为测试`companion.rs`：开启既有安全service warning、明确shadow_scan、补live claim/rejected marker计数；内层timeout/panic均在cleanup前最多1秒脱敏取证，并用真实PG验证计数和隐私/错误隐藏。生产逻辑、默认shadow、政策、25/45秒期限、原机会与默认并发保持。
- 当前669输入匹配冻结manifest，SHA为`27d16d5b28bc4b0dcc60a92b2522d72a330fb56e2da9a1ace2483835ad769093`，同669提交blob完全一致；限定Rust格式和源码/新写文档diff检查通过（原样证据4处空白按哈希保留，见HANDOFF索引）。恢复收口复核沿用原结果，未新跑第四批。
- 外层45秒直接drop、初始化和cleanup自身失败没有完整阶段见证；本轮没有动态注入整条paired失败清理，不能声称新增摘要覆盖所有失败。

## 资源与证据

[归档索引](handoffs/2026-10-15-v01/evidence-index.json)保存原日志/JSON/脚本，二进制保留在ignored `run/validation-20261015-v01/`并有哈希收据。没有Linux应用、真实模型、录放音、执行器或长稳验收。

任务隔离PG容器/卷已移除，当前相关测试进程为空；全量结束当刻schema/连接快照未保存，字段保持unknown。恢复时共享OrbStack停止，Root启动它用于核对并回收已停止的任务容器；没有重启PG，astrbot随共享runtime恢复运行并保留。baseline资源0/0仅为两批基线后的独立事实，不能代替最终即时快照。

## 下一条可执行工作

1. **V01有界调查已收口，根因待新证据。** 只有出现新的失败、原V3原件或能区分候选的运行条件，才再固定源码/binary/环境和有限批次定位；优先保留cleanup前phase、shadow/live计数、pending/admission reason、原预算与PG等待。无证据不延长期限、不刷新机会、不降低并发、不扩日志框架。
2. 如V01再次出现，先区分Ready/seed、live discovery、freeze/admit、reserve/output、TTS/media、cleanup阶段。当前匿名并发日志不能按顺序绑定场景，pool/query总耗时也不能当作单独pool等待。
3. 下一独立实施包由用户选择既有软件/部署待办；不把本轮未复现转为扩大无人值守live的许可。默认shadow继续保持。
4. R02硬件AEC优先已确认，R1板/固件、厂商附件及输出/电气资料仍待；R03后续暂停。R05需真实音频、机构与部署出口，前端/通用适配/端上或同机部署/其它平台仍按[延期台账](deferred.md)。
