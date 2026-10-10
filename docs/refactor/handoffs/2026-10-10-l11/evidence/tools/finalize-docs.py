from pathlib import Path
import json
r = Path(__file__).resolve().parent
root = r.parents[1]
d = root / 'docs/refactor'
h = d / 'handoffs/2026-10-10-l11'
v = json.loads((r / 'commit-verification.json').read_text())
source, sha = v['source_commit'], v['manifest_sha256']
assert v['matching_git_blobs'] == v['input_count'] == 640 and not v['mismatches']
e = h / 'evidence'
mac = json.loads((e / 'macos/workspace-v4.counts.json').read_text())
linux = json.loads((e / 'linux/server-tests-repeat.counts.json').read_text())
assert mac['totals']['passed'] == 1060 and linux['totals']['passed'] == 778
assert mac['assessment'] == linux['assessment'] == 'PASSED_RECORDED_RUN'
closure = json.loads((e / 'resource-closure.json').read_text())
assert closure['all_task_containers_stopped'] and not closure['remaining_test_schemas'] and closure['other_database_connections'] == 0
removed = len(closure['leftover_schemas_removed'])
cleanup = f'关闭前清理 {removed} 个被杀 child 遗留的任务 schema，最终 schema/其他连接均为零。' if removed else '停机前 schema/其他连接均为零，无需额外删除 schema。'
summary = f'源码 `{source}`，V4 manifest `{sha}`，640/640 Git blob 一致。macOS workspace **1060/0/5**、真实 PTY **3/0**、无默认 availability **2/0**、提前准入 **1/0**；Linux ARM64 Server 完整默认并行复跑 **778/0/5**，两端无默认全目标及 Linux release/native link 通过。所有最终结果同源且无漂移。'
limits = '未运行 Linux 全 workspace 或部署 smoke，未验证真实模型、Pi/声学/机械与其他平台。Linux V4 首次旧 panic-fence 子进程 20 秒超时仍保留为未定位项；定向和完整复跑通过不等于根因修复。'
footer = f'\n## Root 最终验收\n\n{summary}\n\n本轮 10/10，L09—L12 3/4，R01—R06 仍 2/6。四个任务容器均停止。{cleanup}原失败、修正和最终证据见本目录 evidence-index.json。\n\n{limits}\n'
for filename, old, new in [('interaction-HANDOFF.md', '6/7', '7/7'), ('companion-HANDOFF.md', '5/6', '6/6'), ('context-HANDOFF.md', '7/8', '8/8')]:
    p = h / filename
    p.write_text(p.read_text().replace(old, new).replace('- [ ]', '- [x]') + footer)
p = h / 'root-HANDOFF.md'
s = p.read_text().replace('本轮 checklist 当前 7/10，等待运行结果、文档与提交关闭。', '本轮 checklist 已完成 10/10；最终同源验证与任务资源关闭完成，源码和文档分别正常提交。')
s = s.replace('最终计数、提交与资源关闭在验收后回填。', '最终计数、源码身份与资源关闭见下方 Root 最终验收。')
s = s.replace('完整默认并行复跑结果待回填。', '同一 V4 默认并行完整复跑 778/0/5，通过且无源码漂移。')
s += '\n保留测试二进制：`run/validation-20261010-l11/linux-arm64-v4/runtime/server_runtime-v4`，461182688 bytes，SHA-256 `975ee9ae4cd700f58fc3b3eb8a2f52b3b59bea841614bd1e1ea2574bdf0ba5d6`。该忽略目录中的证据文件未部署或纳入 Git；环境镜像和摘要已归档。Idle Linux holder 停止时的 137 不是测试退出码；各验证阶段退出码另有结果文件。\n'
p.write_text(s + footer)
p = d / 'checklists/l11-2026-10-10.md'
s = p.read_text().replace('当前 **7/10**', '当前 **10/10**').replace('L09—L12 **2/4**', 'L09—L12 **3/4**').replace('- [ ]', '- [x]')
p.write_text(s + '\n' + summary + '\n\n原失败与最终通过记录均保留；Linux 旧关闭测试超时仍为 L12 的可观测性待办，不以复跑消除该事实。下一项 L12，见恢复检查点。\n')
p = d / 'layering-l11-2026-10-10.md'
results = f'''## 最终验证与证据

{summary}

| 验证 | 最终结果 |
| --- | --- |
| macOS 默认 workspace | 1060 passed / 0 failed / 5 ignored |
| macOS 真实 PTY | 3 passed / 0 failed，fd 0/1/2 均为 TTY |
| macOS 无默认 | all-targets check 通过；availability 2/0、提前准入 1/0 |
| macOS 提交故障定向 | 1/0，单列且不累加到 workspace unique |
| Linux ARM64 Server | V4 完整默认并行复跑 778 passed / 0 failed / 5 ignored |
| Linux 无默认 / release / native link | 全部通过；ELF/ldd 与 native archive 摘要归档 |
| 架构守卫 / 计数器自测 | 通过；计数器 8/8 正反例 |

V1→V2 修复 authority 字段可见性，V2→V3 修正测试导入/实例，V3→V4 保留新增测试的模型观察 receiver。V3 两端失败都是新增 fixture 在 SQL 前发生 SendError；V4 仅一份测试文件变化，639/640 输入不变，真实故障到达断言仍在。

Linux V4 首次全量 777/1/5：旧 panic-fence 子进程超过既有 20 秒总期限。相同二进制定向 1/0、16.40 秒通过；再默认并行完整复跑 778/0/5。没有调整期限、串行测试或修改生产。启动 executable digest 计算可能占用期限，但无阶段测量，根因继续开放；细节与保留二进制见 [Root HANDOFF](handoffs/2026-10-10-l11/root-HANDOFF.md)。这独立于原 V01。

原始日志、失败计数、修正差异、640 输入清单与结果摘要见 [证据索引](handoffs/2026-10-10-l11/evidence-index.json)。Cargo stdout/stderr 的两种有界顺序交错由计数器正反例处理，原始字节未重写。嵌套 child 汇总不重复累加，定向复查不增加 unique 数量。

本轮四个容器已停止。{cleanup}用户 5433 未访问；原失败与最终测试阶段退出码分别保留。

{limits}
'''
s = p.read_text().replace('运行结果及源码身份在统一验证完成后补入。提交未知的测试证据仍是控制真实 PostgreSQL 事务提交/回滚的 barrier，不声称物理网络 ACK 丢失。', results + '\n提交未知证据仍是控制真实 PostgreSQL 提交/回滚的 barrier，不声称物理网络 ACK 丢失。')
p.write_text(s)
p = d / 'checklists/layering-next.md'
s = p.read_text().replace('当前 **2 / 4 完成**', '当前 **3 / 4 完成**')
s = s.replace('- [ ] **L11 跨域事务**：确定 Context、Companion、Delivery 跨表校验与写入的所有者；来源有效性、隐私 epoch、Context 冻结与输出提交仍须保持必要的原子性。', '- [x] **L11 跨域事务**：ResponseCommitter 统一 Response/history；Companion 拥有 source/grant/policy，Context 保留私有租约和证据适配，必要原子性保持；见 [本轮 10/10](l11-2026-10-10.md)。')
s = s.replace('L10 已验收，下一步从 L11 开始。', 'L10/L11 已验收，下一步从 L12 开始。').replace('跨域事务事项仍属于 L11，不能随目录搬迁提前勾选。', '跨域事务已按 L11 运行验收，L12 仍须按真实契约和关闭证据单独完成。')
s = s[:s.index('L11 的三条已核实恢复入口')] + 'L11 已完成，下一步核对 Delivery 的 Channel transport 与 TTS audio budget 所有者；同时保留本轮旧 panic-fence 20 秒超时的阶段定位待办。具体恢复入口见 [L11 Root HANDOFF](../handoffs/2026-10-10-l11/root-HANDOFF.md)。不得通过延长期限、串行化或忽略原失败关闭此项。\n'
p.write_text(s)
p = d / 'resume-checkpoint-l11-2026-10-10.md'
p.write_text(f'''# 当前检查点：2026-10-10 L11 完成

分支 `codex/architecture-refactor`；源码 `{source}`。本轮 **10/10**，L09—L12 **3/4**，R01—R06 仍 **2/6**；不是整个项目完成百分比。三个 subagent 已完成交接，无待恢复 Cargo/PG 作业，四个任务容器均停止。

依次读取 [技术说明与验证](layering-l11-2026-10-10.md)、[checklist](checklists/l11-2026-10-10.md)、[四份 HANDOFF](handoffs/2026-10-10-l11/README.md)、[证据索引](handoffs/2026-10-10-l11/evidence-index.json)，再核对实际 Git 状态与 [L12 清单](checklists/layering-next.md)。

ResponseCommitter 拥有规范 Response/history 联合提交；Conversation 删除旧提交入口。Companion 拥有 observation/erasure roots/private grant/policy；Context 保留私有租约签发、规范来源和必要证据适配。Live/Shadow 仍统一提交，Delivery 参与函数仅接原 tx；无人调用的旧状态入口已删除。默认 Shadow、当前政策、双 epoch、锁顺序、两类暂停、输出身份和工具迟到事实保持。

{summary}

V3 的新增 fixture receiver 生命周期错误已在 V4 修复，原故障注入断言未放宽。Linux V4 首次旧 panic-fence 子进程超过 20 秒，定向复查与默认并行全包复跑通过；根因仍开放，需后续阶段耗时/数据库等待证据，不能宣称已修复。保留二进制和 SHA 见 Root HANDOFF。原失败、修正、最终证据均已归档。

**下一项：L12 出站契约。** 先追 `delivery/service.rs`、`service/media.rs` 的 Channel transport 类型/实现/fixtures，再查 `service/output.rs` 对 TTS `ResponseAudioBudget` 的实际所有者与生命周期。保留精确取消、单次派发、预算、真实投递和 native owned 关闭；同步跟踪上述旧关闭测试的可观测性。继续三个互斥 subagent，Root 统一验证，更新 checklist/HANDOFF/检查点并正常提交。

Skills HTTP CRUD 三处 reload TODO、V01、R02 AEC、R03 物理反馈、Pi/24h/R05、其他平台/R06、前端与未来端上部署仍见 [延期台账](deferred.md)。不扩依赖/feature/平台/模型/硬件动作，不访问用户 5433。

{cleanup}用户允许额度不足使用 reset 卡，本轮未使用；未推送或创建 PR。

{limits}
''')
for filename, relative in [('resume-checkpoint-2026-10-10.md', 'resume-checkpoint-l11-2026-10-10.md')]:
    p = d / filename; lines = p.read_text().splitlines()
    lines[1:1] = ['', f'后续 L11 已完成；最新恢复入口见 [L11 检查点]({relative})。以下保留 L10 当时记录。']
    p.write_text('\n'.join(lines) + '\n')
p = d / 'platform-validation.md'; s = p.read_text(); s = s.replace('# 隔离平台验证\n', f'# 隔离平台验证\n\n**2026-10-10 L11：10/10，L09—L12 3/4，六轮仍 2/6。** {summary} Linux 首次旧关闭测试超时仍待定位，原失败与复跑均保留。资源已关闭；见 [当前检查点](resume-checkpoint-l11-2026-10-10.md) 与 [最终验证](layering-l11-2026-10-10.md#最终验证与证据)。以下为历史记录。\n', 1);p.write_text(s)
p = d / 'checklists/next-round.md';s = p.read_text().replace('更新：2026-10-09。','更新：2026-10-10。',1);s=s.replace('## R01 ·', f'2026-10-10 L11 跨域事务所有者清理 **10/10**，源码 `{source[:7]}`；L09—L12 **3/4**，六轮仍 **2/6**。恢复从 [L11 检查点](../resume-checkpoint-l11-2026-10-10.md) 开始，下一项 L12；原 V01 与本轮旧关闭测试超时定位继续保留。\n\n## R01 ·',1);p.write_text(s)
p = d / 'deferred.md';p.write_text(p.read_text() + '''
## 2026-10-10：L11 验收后的软件续接

- [x] L11 跨域事务所有者清理完成，10/10；L09—L12 3/4，六轮仍 2/6，见 [当前检查点](resume-checkpoint-l11-2026-10-10.md)。
- [ ] L12 出站契约：核对 Delivery 的 Channel transport / TTS audio budget 所有者，保持取消、限额和真实关闭证据。
- [ ] Linux 旧 panic-fence 测试子进程 20 秒超时阶段定位：V4 首次 777/1/5，原二进制定向 1/0、16.40 秒，默认并行全包复跑 778/0/5。没有改生产、期限或并行度；启动摘要计算只是候选原因，不宣称已修复。保留 binary/SHA、原失败与复查见 [Root HANDOFF](handoffs/2026-10-10-l11/root-HANDOFF.md)。后续先补阶段耗时和 PG 等待证据，独立于 V01。
- [ ] Skills HTTP CRUD 三处 reload TODO 继续开放。

原 V01、R02/R03/R05/R06、前端、真实模型、硬件与未来端上部署的状态不变；无新增硬件或部署授权。
''')
print(json.dumps(dict(source_commit=source, checklist='10/10', layering='3/4', rounds='2/6', closed=True)))
