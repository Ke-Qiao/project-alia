from pathlib import Path
import json
r=Path(__file__).resolve().parent
root=r.parents[1]
ref=root/'docs/refactor'
hand=ref/'handoffs/2026-10-11-settings-recovery'
summary=json.loads((r/'final-stage-summary.json').read_text())
commit=json.loads((r/'commit-verification.json').read_text())['source_commit']
closure=json.loads((r/'resource-closure.json').read_text())
assert closure['all_task_containers_stopped'] and not closure['remaining_test_schemas']
def counts(name):
 c=summary['counts'][name];return f"{c['passed']}/{c['failed']}/{c['ignored']}"
mac=counts('macos_workspace');linux=counts('linux_server');sha=summary['manifest_sha256']
record=f'''源码 `{commit}`；冻结 V1 manifest `{sha}`，{summary['inputs']}/{summary['inputs']} 源码 Git blob 一致。

macOS ARM64 workspace **{mac}**；Linux ARM64 Server 默认并行完整回归 **{linux}**；两端 Server 无默认 feature 全目标检查、架构边界正反自测、变更范围 rustfmt 和差异检查通过。数字为 passed/failed/ignored，嵌套子进程摘要不重复累加。全部最终阶段使用同一份源码，没有源码漂移。本轮新增 12 项（7 storage、2 Registry、3 HTTP/runtime）在两端通过；没有首次失败后隐藏重跑或放宽测试期限。

全部 Cargo 阶段结束。本轮独立 PostgreSQL 与 Linux holder 已停止，关闭前其他 DB 连接为零，测试 schema 为零，遗留 schema 清理数 {len(closure['leftover_schemas_removed'])}；用户 5433 未访问。日志、源快照及缓存保留在 ignored `run/validation-20261011-settings-recovery/`，可核对副本见证据索引。空闲 holder 的停止码不作为 Cargo 结果。
'''
limits='未运行真实断电、独立进程 kill/restart、真实 PTY／Easy-Turn、Linux 全 workspace、release／部署 smoke、外部付费服务、Pi／声学／执行器或长稳。存储回归使用软件故障注入，runtime 重启使用实际 ServerBuilder；不等于操作系统进程重启或物理断电。ACL／扩展属性迁移、异 owner 拒绝的提权实测、FIFO 故障和其他本机进程并发改写没有验证。'
next_step='下一软件候选：先审计纯非政策 `PATCH /api/v1/settings` 在等待更新锁／政策读取期间发生管理员降权时的准入重验证，保留现有 SQL actor 核验、API 与权限政策；先复现，再补缺口。'
for p in hand.glob('*-HANDOFF.md'):
 text=p.read_text()
 text=text.replace('- [ ] Root','- [x] Root')
 text=text.replace('最终验收与提交 **0/3**，以 Root 后续补记为准。','最终验收与提交现为 **3/3**；实施阶段待验收状态由下方 Root 最终记录关闭。')
 text=text.replace('当前本分工 **6/8**。','实施交接时为 **6/8**，Root 最终验收后为 **8/8**。')
 text+='\n## Root 最终验收补记\n\n'+record+'\n本轮总 checklist **10/10**，L09—L12 仍 **4/4**，R01—R06 仍 **2/6**，不是项目完成百分比。前文实施时点的未运行说明保留，实际整轮结果以本节为准。\n\n'+limits+'\n'
 p.write_text(text)
p=hand/'README.md';p.write_text(p.read_text()+'\n本轮 **10/10**。\n\n'+record)
p=ref/'checklists/settings-recovery-2026-10-11.md';text=p.read_text().replace('本轮实施中；','本轮 **10/10 完成**；').replace('- [ ]','- [x]');p.write_text(text+'\n'+record+'\n'+next_step+'\n\n恢复入口：[当前检查点](../resume-checkpoint-settings-recovery-2026-10-11.md)。\n')
p=ref/'settings-recovery-2026-10-11.md';p.write_text(p.read_text()+'\n## 最终验收\n\n'+record+'\n'+limits+'\n')
checkpoint=f'''# 当前检查点：2026-10-11 运行时 Settings 恢复完成

分支 `codex/architecture-refactor`，本轮 **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**，不是项目完成百分比。三个 subagent 已完成交接，无待恢复 Cargo／PG 作业。

依次读取 [技术说明](settings-recovery-2026-10-11.md)、[checklist](checklists/settings-recovery-2026-10-11.md)、[四份 HANDOFF](handoffs/2026-10-11-settings-recovery/README.md) 与 [证据索引](handoffs/2026-10-11-settings-recovery/evidence-index.json)，再核对 branch/status/log。

{record}
Settings 域独占保存 gate，准入后取快照，私有候选→file sync→原子 rename→完整 canonical 父链 sync；发布前失败保旧，发布后失败明确持久性未确认。保留 desired／module／SQL 部分成功和同值显式重试。运行时设置没有 setup 式 journal；失败后重启加载完整 old/new 文件，SQL 是政策权威。首次 setup 原受约束恢复不变。普通加载只读；硬链接／无法保持 owner/group 的替换拒绝，不提权。

用户明确确认本轮 privacy PATCH 统一 SQL 政策和管理员修改权限；GET 登录权限、请求字段和原 8 字段响应保留。两个 PATCH 共享 SQL current／If-Match／CAS，privacy GET/audit 不再读取陈旧 RAM。SQL 失败不更新内存投影，privacy 不再保存被过滤的政策文件。

**{next_step}**

V01、R02 AEC、R03 反馈、Pi/24h/R05、R06、前端和可信文件系统部署等暂缓项保持开放，见 [台账](deferred.md)。不要由本轮软件通过推断真机完成。

未推送、未建 PR，未使用 reset 卡。

{limits}
'''
(ref/'resume-checkpoint-settings-recovery-2026-10-11.md').write_text(checkpoint)
p=ref/'resume-checkpoint-config-durability-2026-10-11.md';text=p.read_text();lines=text.splitlines();lines.insert(2,'后续运行时 Settings 轮已完成，最新恢复从 [Settings 检查点](resume-checkpoint-settings-recovery-2026-10-11.md) 开始；下文保留配置轮历史状态。');p.write_text('\n'.join(lines)+'\n')
p=ref/'checklists/next-round.md';p.write_text(p.read_text()+f'\n## 2026-10-11 运行时 Settings 续轮\n\n本轮 **10/10**，源码 `{commit[:7]}`；六轮仍 **2/6**。原子快照与明确失败阶段、同值恢复、privacy SQL／管理员入口和同源双平台验收已完成。最新恢复从 [Settings 检查点](../resume-checkpoint-settings-recovery-2026-10-11.md) 开始。{next_step}\n')
p=ref/'checklists/management.md';p.write_text(p.read_text()+f'\n## 运行时 Settings 恢复续轮\n\n- [x] 源码 `{commit[:7]}`，原子文件保存、明确发布失败阶段及显式重试；privacy 按用户确认统一 SQL 政策与管理员修改，保留成功响应形状与原读取权限。见 [本轮 10/10](settings-recovery-2026-10-11.md)。\n- [ ] {next_step}\n')
p=ref/'deferred.md';text=p.read_text();old='- [ ] 下一软件候选：先审计 `SettingsRegistry::save` 与 `PATCH /api/v1/settings` 的 live apply、内存配置及 `settings.toml` 持久化顺序，定义保存失败与重试的恢复语义，再在授权范围补缺口。保持 API、当前政策和权限；先检查既有实现，不直接扩展本轮发布协议为通用事务框架。';assert old in text;text=text.replace(old,'- [x] 后续 [Settings 轮](resume-checkpoint-settings-recovery-2026-10-11.md) 已完成运行时保存、部分成功与重试语义，并按用户确认统一 privacy SQL／管理员入口。');text+='\n## 2026-10-11 Settings 恢复后的续接\n\n- [x] 本轮 10/10，源码 `'+commit[:7]+'`，同源双平台软件验收完成。\n- [ ] '+next_step+'\n- [ ] 任意 ACL／扩展属性保留、跨进程共享 settings 文件协调及真实存储/断电保证仍需部署需求和独立验证。本轮没有扩大平台或依赖。\n\n'+limits+'\n';p.write_text(text)
p=ref/'platform-validation.md';p.write_text(p.read_text()+'\n## 2026-10-11 Runtime Settings 恢复验收\n\n'+record+'\n详见 [交接](handoffs/2026-10-11-settings-recovery/README.md)。\n\n'+limits+'\n')
print(json.dumps({'source_commit':commit,'macos':mac,'linux':linux,'checklist':'10/10'}))
