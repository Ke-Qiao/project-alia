from pathlib import Path
import json
r=Path(__file__).resolve().parent
root=r.parents[1]
d=root/'docs/refactor';h=d/'handoffs/2026-10-11-health'
v=json.loads((r/'commit-verification.json').read_text());s=json.loads((r/'final-stage-summary.json').read_text());c=json.loads((r/'resource-closure.json').read_text())
assert v['manifest_sha256']==s['manifest_sha256'] and v['matching_git_blobs']==v['input_count'] and not v['mismatches']
assert c['all_task_containers_stopped'] and c['other_database_connections']==0 and not c['remaining_test_schemas']
commit=v['source_commit'];sha=v['manifest_sha256'];n=v['input_count']
mac=s['stages']['macos/workspace-v2'];linux=s['stages']['linux/server-tests']
def counts(stage):
 x=stage['counts'];return '/'.join(str(x[k]) for k in ['passed','failed','ignored'])
identity=f'源码 `{commit}`，V2 manifest `{sha}`，{n}/{n} Git blob 一致。'
results=f'macOS workspace **{counts(mac)}**；Linux ARM64 Server 默认并行完整回归 **{counts(linux)}**。两端无默认 feature 全目标检查、边界正反自测、变更文件格式检查和计数器 8 例自测全部通过。数字依次为 passed/failed/ignored，嵌套子进程摘要不重复累计；所有最终阶段同源、无源码漂移。'
closure=f'所有 Cargo 阶段结束；关闭前其他数据库连接为零，最终测试 schema 为零（遗留 schema 清理数 {len(c["leftover_schemas_removed"])}）。V1/V2 Linux holder 与任务 PostgreSQL 已停止，用户 5433 未访问。空闲 holder 的 sleep 容器停止码不作为测试结果；V2 验证命令均 exit 0，V1 编译失败 exit 101 另行保留。缓存与只读快照保留在 ignored `run/validation-20261011-health/`。'
next_task='下一软件任务：配置目录持久性。先核对 base 文件与 fragments 的创建、同步和原子发布调用链，定义发布后父目录 sync 失败的未知结果、目录树/祖先同步和恢复规则，再补必要实现与隔离文件系统回归。保持当前 API、政策、权限与已有依赖；真实断电不能用进程 kill 代替。本轮未实施这项。'
limits='未运行 Linux 全 workspace、release/native release link、部署 smoke、真实 PTY、外部或付费模型、Pi/声学/执行器及长稳验证。6 秒预算约束合作式异步检查，不能强制中断同步阻塞；本地取消不代表远端立即停止处理请求。'
failure='V1 两端首次编译均因聚合 stream 内 async mapper 的高阶借用生命周期泛化失败而 exit 101，没有测试通过数字。确认两端退出后，V2 仅把 mapper 改为先构造借用 Future Vec 再进入 buffer_unordered，期限、并发、取消和测试逻辑均未改变；两个版本清单与原始日志全部保留。'
# Technical note and checklist.
p=d/'health-2026-10-11.md';text=p.read_text().replace('最终运行数字、冻结身份与提交将在统一验收后补充。',f'''### 最终验证与证据

{identity}

{results}

| 最终命令 | 平台 | 结果 / 秒 |
| --- | --- | --- |
| `cargo test --workspace --offline --locked` | macOS ARM64 | {counts(mac)} / {mac['elapsed_seconds']} |
| `cargo test -p alia-server --offline --locked` | Linux ARM64 | {counts(linux)} / {linux['elapsed_seconds']} |
| `cargo check -p alia-server --no-default-features --all-targets --offline --locked` | macOS ARM64 | exit 0 / {s['stages']['macos/no-default-v2']['elapsed_seconds']} |
| 同上 | Linux ARM64 | exit 0 / {s['stages']['linux/server-no-default']['elapsed_seconds']} |

{failure}

{closure}

完整原始日志、环境与命令、两版输入、Git blob 核对和资源状态见 [证据索引](handoffs/2026-10-11-health/evidence-index.json)。5 项 ignored 原因在原始日志和计数明细单列，不以本轮通过替代真实 PTY/模型证据。

{limits}''');p.write_text(text)
p=d/'checklists/health-2026-10-11.md';text=p.read_text().replace('本轮实现完成，等待统一运行验收','本轮 **10/10 完成**').replace('- [ ]','- [x]');p.write_text(text+f'\n{identity}\n\n{results}\n\n{closure}\n\n{failure}\n\n{next_task}\n\n恢复入口：[当前检查点](../resume-checkpoint-health-2026-10-11.md)。\n')
# Per-owner handoffs retain implementation-stage evidence and append final root results.
for name in ['registry','provider','regression']:
 p=h/(name+'-HANDOFF.md');text=p.read_text()
 if name=='registry':text=text.replace('- [ ] Root 编译、真实 HTTP／Provider 验证、同源双平台回归及最终提交；当前实现完成不代表测试通过。','- [x] Root 完成 V2 同源双平台验收与源码提交，最终记录见下文。')
 if name=='regression':
  text=text.replace('- [ ] Root 在统一冻结输入上完成编译、定向及完整运行，并记录实际结果。','- [x] Root 在统一 V2 冻结输入上完成默认完整回归（含本轮三项）及无默认全目标检查。').replace('- [ ] Root 同源验收、资源清理、checklist／恢复点和正常提交。','- [x] Root 同源验收、资源清理、checklist／恢复点和正常提交。').replace('当前本分工 **6/8**。','实现交接时本分工为 **6/8**，现经 Root 验收收口为 **8/8**。')
 text+=f'\n## Root 最终验收补记\n\n{identity}\n\n{results}\n\n{failure}\n\n{closure}\n\n所有 Arc 身份断言在 V1 冻结前已移入既有 catch／stop_runtime 保护区域；运行通过仍以 V2 完整日志为准。上文“尚未运行”描述 subagent 交接当时状态，此处为后续实际验收。\n\n{limits}\n'
 p.write_text(text)
(h/'root-HANDOFF.md').write_text(f'''# /health 清理 Root HANDOFF

承接 `d27490f`。三个 subagent：`skills_registry` 负责 Registry 聚合与内部测试，`skills_http` 负责 Router/Gemini，`skills_regression` 负责真实 HTTP 回归。Root 负责 Builder profile 所有权、交叉整合、唯一 Cargo/PostgreSQL/Docker 验证窗口和正常提交。三个 subagent 已完成交接。

本轮 **10/10 完成**，L09—L12 保持 **4/4**，R01—R06 保持 **2/6**，这些数字不是项目整体完成百分比。实现见 [技术说明](../../health-2026-10-11.md)。

核心闭环：一轮 6 秒绝对期限／8 项并发 → Registry 单轮准入 → 实际实例去重映射回角色 → 保留完成结果 → 超时、drop、stop 回收 future。相同配置 profile 共用一个 lifecycle owner，custom override 独立；Router 不递归探测；Gemini 补独立 5 秒期限、配置快照与安全错误。API、默认 probe=false、Shadow、工具可见性与执行批准保持，无新增依赖、feature、平台或硬件动作。

{failure}

{identity}

{results}

{closure}

## 恢复步骤

1. 读取 [当前检查点](../../resume-checkpoint-health-2026-10-11.md)、checklist 与其余三份 HANDOFF，核对 branch/status/log 和 [证据索引](evidence-index.json)。
2. 本轮没有待恢复验证作业或未完成实现。V1 是失败编译输入，V2 是最终验收输入；新变更另冻清单，不能覆盖历史证据。
3. {next_task}
4. 继续三个职责互斥的 subagent，Root 统一冻结与验收；原 V01、R02/R03/R05/R06 与 D/H 门槛见 [延期台账](../../deferred.md)，不扩大硬件或部署动作。

未推送／未建 PR，本轮未使用 reset 卡。

{limits}
''')
(h/'README.md').write_text(f'''# 2026-10-11 /health 交接

本轮 **10/10**。{identity}

[Root](root-HANDOFF.md) · [Registry](registry-HANDOFF.md) · [Provider/Router](provider-HANDOFF.md) · [HTTP 回归](regression-HANDOFF.md)。

[技术说明](../../health-2026-10-11.md) · [checklist](../../checklists/health-2026-10-11.md) · [恢复检查点](../../resume-checkpoint-health-2026-10-11.md) · [证据索引](evidence-index.json)。

{results}

{failure}

{closure}
''')
(d/'resume-checkpoint-health-2026-10-11.md').write_text(f'''# 当前检查点：2026-10-11 /health 清理完成

分支 `codex/architecture-refactor`；本轮 **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**，不是整个项目完成百分比。三个 subagent 已交接，没有待恢复的 Cargo/PG 作业。

依次读取 [技术说明](health-2026-10-11.md)、[checklist](checklists/health-2026-10-11.md)、[四份 HANDOFF](handoffs/2026-10-11-health/README.md) 和 [证据索引](handoffs/2026-10-11-health/evidence-index.json)，然后核对 Git branch/status/log。

{identity}

整体 health 改为 6 秒绝对预算、最多 8 项并发，同 Registry 一轮准入；重叠请求立即返回原字段的 busy 降级，未启动额外 probe。实际同实例角色共享结果，同名异实例独立。配置同 profile 的 LLM 角色共用 Arc 与 init/start/settings/stop owner；显式 override 保持独立，失败回滚保持。Router 只检查本地模式和依赖绑定；OpenAI 既有实现复用，Gemini 补 5 秒期限、锁释放与安全错误。

已完成结果保留，未完成项超时或停止降级；调用者 drop／stop 不留下无主 probe，停止后不再准入。API、默认 probe=false、当前政策保持，无新增依赖、feature、配置、平台或硬件动作。没有新增结果缓存或深度诊断端点。

{results}

{failure}

**{next_task}**

可信本机文件系统的部署账户、权限和路径仍开放；原 V01、R02 AEC、R03 反馈、Pi/24h/R05、其他平台/R06 与用户暂缓项见 [延期台账](deferred.md)。本轮没有修改常规模型生成中的原有配置锁等待语义；不同 profile 指向同 endpoint 仍独立。

{closure}

未推送／未建 PR，本轮未使用 reset 卡。{limits}
''')
# Current pointers retain historical statements instead of rewriting previous results.
p=d/'resume-checkpoint-classifier-2026-10-11.md';text=p.read_text();lines=text.splitlines();lines[2:2]=['后续更新：本轮之后的 /health 整体预算、重复探测和并发边界已完成，最新恢复从 [Health 检查点](resume-checkpoint-health-2026-10-11.md) 开始；下文保留 Classifier 轮当时状态。',''];p.write_text('\n'.join(lines)+'\n')
p=d/'checklists/layering-next.md';text=p.read_text();start=text.index('后续更新：');end=text.index('\n',start);text=text[:start]+'后续更新：Skills HTTP CRUD reload、Classifier 运行目录衔接和 /health 清理均已完成（各轮 10/10），下一项配置目录持久性见 [最新检查点](../resume-checkpoint-health-2026-10-11.md)。下方旧阶段的“下一项”保留历史语境。'+text[end:];p.write_text(text)
p=d/'checklists/next-round.md';text=p.read_text();anchor='## R01 ·';entry=f'2026-10-11 /health 清理 **10/10**，源码 `{commit[:7]}`；六轮仍 **2/6**。整体预算、重复探测、并发和生命周期已同源双平台验收。最新恢复从 [Health 检查点](../resume-checkpoint-health-2026-10-11.md) 开始，下一软件项为 R04 配置目录持久性。\n\n';text=text.replace(anchor,entry+anchor);p.write_text(text)
p=d/'checklists/management.md';text=p.read_text();old='- [ ] **整体 health 限额**：在保留 API 的前提下明确正常巡检/深度诊断、聚合预算、同 profile 去重、并发上限；如需要缓存，绑定配置 revision 与新鲜度。当前只具有单次 OpenAI probe 上限。';new=f'- [x] **整体 health 限额**：2026-10-11 源码 `{commit[:7]}`，保留 API/default probe=false，6 秒总预算／8 并发／同 Registry 单轮准入，相同 profile 共享实例，Router 本地检查，Gemini 5 秒有界与脱敏；无缓存或新增诊断入口。实际同源验收见 [本轮 10/10](health-2026-10-11.md)。';assert old in text;text=text.replace(old,new).replace('后面三项为本次审查明确保留的 R04 增强/部署出口','后面三项是 2026-10-04 审查记录的 R04 增强/部署出口，其中 health 已在 2026-10-11 单独验收，其余两项继续开放');p.write_text(text)
p=d/'deferred.md';text=p.read_text().replace('- [ ] **R04**：整体 `/health` 预算/并发/重复探测和配置目录断电持久性仍开放，见 [management checklist](checklists/management.md)。','- [ ] **R04**：整体 `/health` 预算/并发/重复探测已由 [Health 轮](resume-checkpoint-health-2026-10-11.md) 验收；配置目录断电持久性和可信本机文件系统部署边界仍开放，见 [management checklist](checklists/management.md)。')
old='- [ ] 下一软件任务：从已记录的 R04 整体 health 限额开始，审计 `api/health.rs`、ModuleRegistry 顺序聚合、Router 重复 minor probe 及 Provider 诊断；先检查现有实现，再收敛整体预算、同 profile 重复探测和并发边界。保持 API、默认 probe=false 和当前政策，不自行启用外部或付费探测。本轮未实施。';assert old in text;text=text.replace(old,'- [x] 后续已由 [Health 轮](resume-checkpoint-health-2026-10-11.md) 完成整体 health 限额、同 profile 去重及并发边界；原 Classifier 轮未实施，历史 scope 保留。')
text+=f'\n## 2026-10-11 Health 清理后的续接\n\n- [x] 本轮 10/10，源码 `{commit[:7]}`；公开 API/default probe=false/政策保持，聚合、实例所有权和 Provider 边界同源验收，见 [检查点](resume-checkpoint-health-2026-10-11.md)。\n- [ ] {next_task}\n- [ ] 可信本机文件系统的账户、权限、路径和其他进程修改边界仍须实际部署固定。\n\n{limits}\n';p.write_text(text)
p=d/'platform-validation.md';text=p.read_text();lines=text.splitlines();lines[2:2]=[f'**2026-10-11 Health：10/10，L09—L12 4/4，六轮仍 2/6。** {identity} {results} 当前结果见 [技术说明](health-2026-10-11.md#最终验证与证据)。V1 两端编译失败记录保留，V2 为最终同源验收；macOS 26.6.2 ARM64 / Debian 12 ARM64、Rust 1.95.0。{limits} 以下为各旧源码历史证据。',''];p.write_text('\n'.join(lines)+'\n')
print(json.dumps(dict(source_commit=commit,manifest=sha,mac=counts(mac),linux=counts(linux))))
