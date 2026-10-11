from pathlib import Path
import json
r=Path(__file__).resolve().parent
ref=r.parents[1]/'docs/refactor'
hand=ref/'handoffs/2026-10-12-settings-authorization'
s=json.loads((r/'final-stage-summary.json').read_text())
c=json.loads((r/'commit-verification.json').read_text())
z=json.loads((r/'resource-closure.json').read_text())
assert z['all_task_containers_stopped'] and not z['remaining_test_schemas']
assert c['matching_git_blobs']==s['inputs']
commit=c['source_commit']
def counts(name):
 t=s['counts'][name]
 return f"{t['passed']}/{t['failed']}/{t['ignored']}"
record=f"""源码 `{commit}`；最终 V2 manifest `{s['manifest_sha256']}`，{s['inputs']}/{s['inputs']} 源码 Git blobs 一致。

macOS ARM64 workspace **{counts('macos_workspace')}**；Linux ARM64 Server 默认完整回归 **{counts('linux_server')}**。两端 Server 无默认 feature 全目标检查、架构边界正反自测、变更 Rust 格式、差异和计数器 8 例自测通过。数字为 passed/failed/ignored，嵌套子进程摘要不重复计入。全部最终阶段同源且无漂移，保持默认测试并发和原期限。新增 8 项身份回归均在两端全量中通过，V1 定向 8/0/0 不额外加到全量数字。

RED 首次屏障失败与 V2 有效越权复现分别保留；GREEN V1 定向通过后，仅修复测试 panic 清理再冻结 V2。Linux 首次准备缺少空目录，复制源码和启动容器前失败；补目录后准备成功，未变更生产源码。最终 Cargo 阶段均通过。

全部 Cargo 已结束，本轮 PostgreSQL 和 Linux holder 已停止。关闭前其他 DB 连接 0、测试 schema 0，遗留 schema 清理 {len(z['leftover_schemas_removed'])} 个；用户 5433 未访问。原日志、源码快照与缓存留在 ignored `run/validation-20261012-settings-authorization/`，可核对副本及哈希见证据索引。空闲 holder 停止码不是 Cargo 结果。
"""
limits='未独立动态验证纯 update gate 排队或慢 JSON body 窗口；最终核验位置由静态顺序与真实 policy/preview 等待验证。未运行真实 PTY/Easy-Turn、Linux 全 workspace、release/部署 smoke、外部付费服务、Pi/声学/执行器或长稳。最终核验是一次准入，准入后的降权不追溯撤销已开始的更新。'
next_step='下一软件候选：先审计并复现 `GET /api/v1/settings` 等待后返回含敏感配置的设置前的当前身份有效性；保持原 API 和管理员读取边界，不扩大为全系统持续撤权框架。'
for p in hand.glob('*-HANDOFF.md'):
 text=p.read_text().replace('- [ ]','- [x]')
 lines=text.splitlines();lines.insert(2,'最终检查项 **8/8 完成**；以下过程说明保留各实施时点，最终结果以文末 Root 验收记录为准。');text='\n'.join(lines)+'\n'
 assert '## Root 最终验收补记' not in text
 p.write_text(text+'\n## Root 最终验收补记\n\n'+record+'\n本轮总 checklist **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**，不是项目完成百分比。前文记录实施时点，已由本节关闭最终验证、资源、源码提交待项。\n\n'+limits+'\n')
p=hand/'README.md';p.write_text(p.read_text()+'\n本轮 **10/10**。\n\n'+record+'\n'+limits+'\n')
p=ref/'checklists/settings-authorization-2026-10-12.md'
p.write_text(p.read_text().replace('本轮正在执行；','本轮 **10/10 完成**；').replace('- [ ]','- [x]')+'\n'+record+'\n'+next_step+'\n\n[恢复检查点](../resume-checkpoint-settings-authorization-2026-10-12.md)。\n')
p=ref/'settings-authorization-2026-10-12.md';p.write_text(p.read_text()+'\n## 最终验收\n\n'+record+'\n'+limits+'\n')
(ref/'resume-checkpoint-settings-authorization-2026-10-12.md').write_text(f'''# 当前检查点：2026-10-12 Settings 身份重验证完成

分支 `codex/architecture-refactor`，本轮 **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**，不是项目完成百分比。三个 subagent 已交接；无待恢复 Cargo/数据库作业。

依次读取 [技术说明](settings-authorization-2026-10-12.md)、[checklist](checklists/settings-authorization-2026-10-12.md)、[四份 HANDOFF](handoffs/2026-10-12-settings-authorization/README.md) 和 [证据索引](handoffs/2026-10-12-settings-authorization/evidence-index.json)，再核对 branch/status/log。

{record}
纯非政策 PATCH 在 gate/current/两次 preview 之后、desired/module/live/file 之前重新核验 active admin，2 秒有界。已降权/停用/删除为403；查询错误/超时为固定脱敏503，无本请求设置副作用。SQL current 自动暂停到期是独立行为，保持原样。政策/embedding 原 SQL actor/CAS、API、默认 shadow、工具批准、权限、精确取消、存储恢复和显式重试均不变，没有新依赖或平台。

**{next_step}**

V01、R02 AEC、R03反馈、Pi/24h/R05、R06、前端和可信文件系统部署仍按 [台账](deferred.md) 开放，不由本轮软件通过关闭。未推送、未建 PR、未使用 reset 卡。

{limits}
''')
p=ref/'resume-checkpoint-settings-recovery-2026-10-11.md'
lines=p.read_text().splitlines();lines.insert(2,'后续身份重验证轮已完成，最新恢复从 [Settings 身份检查点](resume-checkpoint-settings-authorization-2026-10-12.md) 开始；下文保留上轮历史状态。');p.write_text('\n'.join(lines)+'\n')
old='- [ ] 下一软件候选：先审计纯非政策 `PATCH /api/v1/settings` 在等待更新锁／政策读取期间发生管理员降权时的准入重验证，保留现有 SQL actor 核验、API 与权限政策；先复现，再补缺口。'
for name,link in [('checklists/management.md','settings-authorization-2026-10-12.md'),('deferred.md','resume-checkpoint-settings-authorization-2026-10-12.md')]:
 p=ref/name;text=p.read_text();assert old in text
 text=text.replace(old,f'- [x] 纯非政策 PATCH 等待后身份核验已完成，源码 `{commit[:7]}`，见 [本轮10/10]({link})。')
 p.write_text(text+'\n## 2026-10-12 Settings 身份重验证后续\n\n- [ ] '+next_step+'\n\n'+limits+'\n')
p=ref/'checklists/next-round.md';p.write_text(p.read_text()+f'\n## 2026-10-12 Settings 身份重验证续轮\n\n本轮 **10/10**，源码 `{commit[:7]}`，六轮仍 **2/6**。纯非政策 PATCH 最终准入与 8 项真实 HTTP/PG 回归已同源双平台验收。最新恢复从 [身份检查点](../resume-checkpoint-settings-authorization-2026-10-12.md) 开始。{next_step}\n')
p=ref/'platform-validation.md';lines=p.read_text().splitlines();lines.insert(2,'**2026-10-12 Settings 身份重验证：10/10。** 当前同源双平台结果见 [最新检查点](resume-checkpoint-settings-authorization-2026-10-12.md)，下方保留历史验证范围。');p.write_text('\n'.join(lines)+'\n\n## 2026-10-12 Settings 身份重验证验收\n\n'+record+'\n'+limits+'\n')
print(json.dumps({'source_commit':commit,'checklist':'10/10','macos':counts('macos_workspace'),'linux':counts('linux_server')}))
