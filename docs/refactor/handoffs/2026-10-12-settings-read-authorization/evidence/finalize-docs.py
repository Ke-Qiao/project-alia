from pathlib import Path
import json,re
r=Path(__file__).resolve().parent
ref=r.parents[1]/'docs/refactor'
hand=ref/'handoffs/2026-10-12-settings-read-authorization'
s=json.loads((r/'final-stage-summary.json').read_text())
c=json.loads((r/'commit-verification.json').read_text())
z=json.loads((r/'resource-closure.json').read_text())
reg=json.loads((r/'authorization-regressions.json').read_text())
assert z['all_task_containers_stopped'] and not z['remaining_test_schemas']
assert c['matching_git_blobs']==s['inputs']
assert all(v=={'get_passed':6,'patch_passed':8} for v in reg['results'].values())
commit=c['source_commit']
def counts(name):
 t=s['counts'][name];return f"{t['passed']}/{t['failed']}/{t['ignored']}"
record=f'''源码 `{commit}`；最终 V3 manifest `{s['manifest_sha256']}`，{s['inputs']}/{s['inputs']} 源码 Git blobs 一致。

macOS ARM64 workspace **{counts('macos_workspace')}**；Linux ARM64 Server 默认完整回归 **{counts('linux_server')}**。两端 Server 无默认 feature 全目标检查、架构边界正反自测、变更 Rust 格式、差异及计数器 8 例自测通过。数字为 passed/failed/ignored，嵌套子进程摘要不重复计入。全部最终阶段同源且无漂移，原期限与默认测试并发保留。新增 GET 6 项及既有 PATCH 8 项均在两端全量中逐项通过；定向 V3 的 6/0/0 不额外计入总数。

RED 在未改生产时有效复现降权后的 200/ETag/假密钥泄露。GREEN V1 为 5/1，V2 保留原断言的诊断单例为 0/1；166 个键中 3 个浮点 value 与直接内存值比较不同，但期望按 HTTP 同样 JSON 编码解码后完整严格相等。V3 只修正测试比较层次，不用容差或删除字段，生产自 V1 起未变。Linux V1 仅准备环境，未执行 Cargo；Linux 最终阶段明确使用 V3。失败、诊断、原 RED 测试全文和各 manifest 均保留。

全部 Cargo 已结束，本轮 PostgreSQL 及 Linux V1/V3 holder 均已停止。关闭前其他 DB 连接 0、测试 schema 0，遗留 schema 清理 {len(z['leftover_schemas_removed'])} 个；用户 5433 未访问。原日志、源码快照与缓存留在 ignored `run/validation-20261012-settings-read-authorization/`，可核对副本与哈希见证据索引。空闲 holder 停止码不是 Cargo 结果。
'''
behavior='GET 最后 Registry 快照等待结束后重新核验 active admin，随后同步构造 JSON/ETag。已降权、停用或删除为403；查询错误/超时为固定脱敏503，不交付设置或ETag。原值/schema/secret/prefix/政策ETag保持；GET原SQL到期转换与RAM政策投影保留。PATCH共享同文件private查询，原位置/期限/消息不变，政策与embedding原SQL actor/CAS未动。没有新增依赖、平台、权限或硬件动作。'
limits='未独立动态注入纯更新 gate 排队或 Registry 私有锁等待；最后核验位置由完整 await 静态审查和真实政策等待验证。未运行真实 PTY/Easy-Turn、Linux 全 workspace、release/部署 smoke、外部付费服务、Pi/声学/执行器或长稳。核验是一次响应准入，不承诺后续持续撤权或撤回已发送响应。'
next_step='下一软件候选：先审计 Settings GET/PATCH 共用 update gate 的排队、各阶段预算和取消所有权。current/final-auth 已有 2 秒，gate/preview/apply 等待尚未故障实证；区分准入前等待与 SQL/desired/module 已发生后的部分成功，不直接套整请求 timeout 或全局回滚，不把客户端断开等同 handler 已取消。'
for p in hand.glob('*-HANDOFF.md'):
 text=p.read_text();assert '## Root 最终验收补记' not in text
 text=text.replace('- [ ]','- [x]')
 n=len(re.findall(r'^- \[x\]',text,re.M))
 lines=text.splitlines();lines.insert(2,f'最终检查项 **{n}/{n} 完成**；以下保留各实施时点，最终结果以文末 Root 验收记录为准。')
 p.write_text('\n'.join(lines)+'\n\n## Root 最终验收补记\n\n'+record+'\n本轮总 checklist **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**，不是项目完成百分比。\n\n'+limits+'\n')
p=hand/'README.md';p.write_text(p.read_text()+'\n本轮 **10/10**。\n\n'+record+'\n'+limits+'\n')
p=ref/'checklists/settings-read-authorization-2026-10-12.md';p.write_text(p.read_text().replace('本轮进行中；','本轮 **10/10 完成**；').replace('- [ ]','- [x]')+'\n'+record+'\n'+next_step+'\n\n[恢复检查点](../resume-checkpoint-settings-read-authorization-2026-10-12.md)。\n')
p=ref/'settings-read-authorization-2026-10-12.md';p.write_text(p.read_text()+'\n## 最终验收\n\n'+record+'\n'+next_step+'\n\n'+limits+'\n')
(ref/'resume-checkpoint-settings-read-authorization-2026-10-12.md').write_text(f'''# 当前检查点：2026-10-12 Settings GET 身份重验证完成

分支 `codex/architecture-refactor`，本轮 **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**，不是项目完成百分比。三个 subagent 已交接，无待恢复 Cargo/数据库作业。

依次读取 [技术说明](settings-read-authorization-2026-10-12.md)、[checklist](checklists/settings-read-authorization-2026-10-12.md)、[四份 HANDOFF](handoffs/2026-10-12-settings-read-authorization/README.md) 和 [证据索引](handoffs/2026-10-12-settings-read-authorization/evidence-index.json)，再核对 branch/status/log。

{record}
{behavior}

**{next_step}**

V01、R02 AEC、R03反馈、Pi/24h/R05、R06、前端和可信文件系统部署仍按 [台账](deferred.md) 开放，不由本轮软件通过关闭。未推送、未建 PR、未使用 reset 卡。

{limits}
''')
p=ref/'resume-checkpoint-settings-authorization-2026-10-12.md';lines=p.read_text().splitlines();lines.insert(2,'后续 GET 身份重验证轮已完成，最新恢复从 [GET 检查点](resume-checkpoint-settings-read-authorization-2026-10-12.md) 开始；下文保留上轮历史状态。');p.write_text('\n'.join(lines)+'\n')
old='- [ ] 下一软件候选：先审计并复现 `GET /api/v1/settings` 等待后返回含敏感配置的设置前的当前身份有效性；保持原 API 和管理员读取边界，不扩大为全系统持续撤权框架。'
for name,link in [('checklists/management.md','settings-read-authorization-2026-10-12.md'),('deferred.md','resume-checkpoint-settings-read-authorization-2026-10-12.md')]:
 p=ref/name;text=p.read_text();assert old in text
 text=text.replace(old,f'- [x] GET Settings 等待后敏感响应准入已完成，源码 `{commit[:7]}`，见 [本轮10/10]({link})。')
 p.write_text(text+'\n## 2026-10-12 Settings GET 后续\n\n- [ ] '+next_step+'\n\n'+limits+'\n')
p=ref/'checklists/next-round.md';p.write_text(p.read_text()+f'\n## 2026-10-12 Settings GET 身份重验证续轮\n\n本轮 **10/10**，源码 `{commit[:7]}`，六轮仍 **2/6**。最后响应准入、GET六项和PATCH八项回归已同源双平台验收。最新恢复从 [GET检查点](../resume-checkpoint-settings-read-authorization-2026-10-12.md) 开始。{next_step}\n')
p=ref/'platform-validation.md';lines=p.read_text().splitlines();lines.insert(2,'**2026-10-12 Settings GET 身份重验证：10/10。** 当前同源双平台结果见 [最新检查点](resume-checkpoint-settings-read-authorization-2026-10-12.md)，下方保留历史验证范围。');p.write_text('\n'.join(lines)+'\n\n## 2026-10-12 Settings GET 验收\n\n'+record+'\n'+limits+'\n')
print(json.dumps({'source_commit':commit,'checklist':'10/10','macos':counts('macos_workspace'),'linux':counts('linux_server')}))
