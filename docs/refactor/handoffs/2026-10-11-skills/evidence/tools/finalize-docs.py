from pathlib import Path
import json
r=Path(__file__).resolve().parent
root=r.parents[1];d=root/'docs/refactor';h=d/'handoffs/2026-10-11-skills'
v=json.loads((r/'commit-verification.json').read_text());s=json.loads((r/'final-stage-summary.json').read_text());c=json.loads((r/'resource-closure.json').read_text())
assert c['all_task_containers_stopped'] and not c['remaining_test_schemas'] and c['other_database_connections']==0
assert v['manifest_sha256']==s['manifest_sha256'] and v['matching_git_blobs']==643 and not v['mismatches']
commit=v['source_commit'];short=commit[:7];manifest=v['manifest_sha256'];st=s['stages']
def count(name):
 x=st[name]['counts'];return f"{x['passed']}/{x['failed']}/{x['ignored']}"
mac=count('macos/workspace-v2');linux=count('linux/server-tests')
summary=f"macOS workspace **{mac}**；Linux ARM64 Server 默认并行完整回归 **{linux}**。两端 Skills lib 定向各 **8/0/0**，无默认全目标检查、边界正反自测与变更文件格式检查通过。计数依次为 passed/failed/ignored，定向测试及嵌套子进程摘要不额外加到完整结果。所有最终结果同源、无源码漂移。"
identity=f"源码 `{commit}`，V2 manifest `{manifest}`，643/643 Git blob 一致。"
limits="未运行 Linux 全 workspace、release/native release link、真实 PTY、部署 smoke、Skills 链路的真实网络 COMMIT ACK 丢失注入、真实模型或硬件验证。本轮完整默认测试包含 native 路径，但不替代上述未运行项目；上轮已有证据只作为历史记录。"
nexttask="下一项软件任务：独立审计静态 classifier 的 skill 发现与 Registry live snapshot 的衔接。读取 `routing/service.rs` 的两份静态模板、`classification_prompt_for_mode` / `classify` / `to_decision`，以及 `routing/mod.rs::RouterInput`、`generation.rs::route_response`，检查新增／禁用／删除 skill 的候选和退化，以及返回技能名的允许范围。先查已有实现再补缺口，保持全局工具可见性与 Execution 批准；本轮未实施。"
validation=f'''{identity}

{summary}

| 最终检查 | 真实命令范围 | 结果 | 秒数 |
| --- | --- | --- | --- |
| macOS workspace | `cargo test --workspace --locked --offline --no-fail-fast` | {mac} | {st['macos/workspace-v2']['elapsed_seconds']} |
| macOS 无默认 | `cargo check --workspace --all-targets --no-default-features --locked --offline` | exit 0 | {st['macos/no-default-v2']['elapsed_seconds']} |
| Linux Server | `cargo test -p alia-server --locked --offline --no-fail-fast` | {linux} | {st['linux/server-tests']['elapsed_seconds']} |
| Linux Server 无默认 | `cargo check -p alia-server --all-targets --no-default-features --locked --offline` | exit 0 | {st['linux/server-no-default']['elapsed_seconds']} |

macOS 26.6.2 arm64 / Rust 1.95.0；Linux Debian 12 ARM64 / Rust 1.95.0，已有镜像和离线缓存。构建 jobs=2，测试使用默认并发、默认栈和既有期限，未设置 `RUST_TEST_THREADS`、`RUST_MIN_STACK`、`DOCS_RS` 或 `RUST_BACKTRACE`。5 项 ignored 仍按原规则保留；无额外 live/终端测试结果混入。

隔离 PostgreSQL `alia-skills-20261011-db` 仅发布到本机 32771，用户 5433 未访问。归档前所有任务连接与 schema 为零，V1/V2 Linux 容器和任务 PG 全部停止；源码快照和缓存保留在 ignored run。原日志、命令、环境、manifest、V1 失败、V2 仅夹具修正、PG 表锁探针和资源关闭见 [证据索引](handoffs/2026-10-11-skills/evidence-index.json)。

{limits}'''
p=d/'skills-2026-10-11.md';t=p.read_text().replace('当前实现冻结为 V2，最终验收与提交身份待 Root 收尾填写；不提前记作通过。',f'本轮 **10/10** 已完成，最终源码 `{short}`。')
a=t.index('待 Root 从完成的命令 sidecar 填写；');b=t.index('\n## 后续恢复范围',a);t=t[:a]+validation+'\n'+t[b:];p.write_text(t)
p=d/'checklists/skills-2026-10-11.md';t=p.read_text().replace('本轮实施中；','本轮 **10/10 完成**；').replace('- [ ] **S','- [x] **S');t+=f'\n最终源码 `{short}`；{summary} 证据及资源关闭见 [技术说明](../skills-2026-10-11.md#最终验证与证据)。\n';p.write_text(t)
(h/'root-HANDOFF.md').write_text(f'''# Skills HTTP 索引清理 Root HANDOFF

本轮 **10/10**，三个 subagent 分别完成 Registry、HTTP、真实回归；Root 统一集成、运行 Cargo/PG/Docker、收尾及提交。L09—L12 **4/4**，R01—R06 **2/6**，不是整个项目完成百分比。

{identity}

## 实现结果

已有 HTTP 领域事务保留；三个 reload TODO 被统一 SkillChange owner 替代。完整 RR snapshot 在提交前准备，提交由 RuntimeTasks 持有，成功后一次发布。准备失败不改变旧索引；提交结果错误先通过表锁等待原事务，再重建实际状态；重建失败时索引不可用，健康 degraded，成功 reload 恢复。HTTP 响应提交前准备，update 读取位于同一事务。等待者取消不丢弃 commit，stop 关闭准入并等待 owned worker。

删除无调用 Registry CRUD/get/list 和 plugin skill facade；边界守卫阻止旧路径恢复。Shadow、工具可见性、执行批准、依赖校验、禁用过滤、请求和错误语义保留。无新增依赖、feature、配置、迁移或硬件动作。

## 验证与真实失败

{summary}

V1 HTTP 3/0/0；V1 macOS/Linux lib 均 7/1/0，失败是 fixture 要求 pool.close 首次 poll Pending。V2 只改一份 registry_tests.rs，允许 Ready/Pending，直接断言 pool.is_closed，确保 worker 完全退出再替换测试依赖；原失败断言的生产目标全部保留。原日志与修正结果分开保存。PG 探针实证先等待表锁再读取 RR snapshot；不声称已注入真实网络 ACK 丢失。

完整命令与逐目标计数在 [证据索引](evidence-index.json)；[技术说明](../../skills-2026-10-11.md#最终验证与证据) 给出平台和时间。所有 Cargo 阶段结束，其他 PG 连接与 schema 为零，两个 Linux holder 和 PG 都已停止。用户 5433 未访问；缓存与只读快照留在 ignored `run/validation-20261011-skills/`。

## 恢复步骤

1. 读取 [恢复检查点](../../resume-checkpoint-skills-2026-10-11.md)、[checklist](../../checklists/skills-2026-10-11.md) 和另外三份 HANDOFF；检查 Git branch/status/log。
2. 本轮没有待恢复作业、未完成实现或隐藏测试。不得重跑旧失败版本来替代最终 V2；新变更冻结为新 manifest。
3. {nexttask}
4. 继续三个互斥 subagent，Root 统一运行和正常提交。原 V01 与 R02/R03/R05/R06 保持开放，前端及未来端上部署按台账执行。

同步仅保证当前 Registry 的成功 HTTP CRUD；外部 SQL/其他实例仍需显式 reload。请求数组元素与 nullable 语义保持，静态路由自动发现未改。默认 Shadow/当前政策保持，无新部署/硬件授权。未推送或建 PR；本轮未使用 reset 卡。

{limits}
''')
(h/'README.md').write_text(f'''# 2026-10-11 Skills HTTP 索引清理交接

本轮 **10/10** 已完成，源码 `{short}`。L09—L12 保持 4/4，六轮保持 2/6，均不代表项目完成百分比。

读取顺序：[Root](root-HANDOFF.md)、[Registry](registry-HANDOFF.md)、[HTTP](http-HANDOFF.md)、[Regression](regression-HANDOFF.md)。三个 subagent 的分工审查与 Root 实际运行结果分开记录。

[技术说明](../../skills-2026-10-11.md) · [checklist](../../checklists/skills-2026-10-11.md) · [恢复检查点](../../resume-checkpoint-skills-2026-10-11.md) · [证据索引](evidence-index.json)。

{summary} V1 原失败与 V2 最终结果分别保留，不混算；任务容器/数据库已停止。
''')
(d/'resume-checkpoint-skills-2026-10-11.md').write_text(f'''# 当前检查点：2026-10-11 Skills HTTP 索引清理完成

分支 `codex/architecture-refactor`；本轮 **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**，不是整个项目完成百分比。三个 subagent 已交接，任务容器/数据库全部停止，无待恢复 Cargo/PG 作业。

依次读取 [技术说明](skills-2026-10-11.md)、[checklist](checklists/skills-2026-10-11.md)、[四份 HANDOFF](handoffs/2026-10-11-skills/README.md) 和 [证据索引](handoffs/2026-10-11-skills/evidence-index.json)，核对 Git 状态后继续。

{identity}

Skills 创建／更新／删除现在在同一变更 owner 下完成 SQL、完整索引准备、owned commit 和一次发布；成功 HTTP 响应之后即反映到当前 Registry 的 match/list/resolve。准备失败保留旧索引；提交失败重建实际状态，重建也失败时停用索引直到 reload 恢复。调用方取消与 stop 不丢弃已交接任务。旧无调用 CRUD/plugin facade 已删除，边界守卫防止回流。

{summary}

V1 两端 lib 7/1/0 的 pool.close 调度假设失败已保留；V2 仅测试夹具修正，不改生产规则或删除失败目标。表锁先于 RR snapshot 的隔离 PG 探针通过；网络 ACK 丢失本身没有端到端故障注入。

**{nexttask}**

同步范围不含其他实例／外部 SQL 的主动通知，它们仍需显式 reload；静态 classifier 尚未自动发现新 skill。默认 Shadow、当前政策、工具可见性和执行批准保持。继续三个互斥 subagent，Root 统一验证与正常提交。

原 V01、R02 AEC、R03 物理反馈、Pi/24h/R05、其他平台/R06 和用户暂缓项见 [延期台账](deferred.md)。不扩依赖/feature/平台/模型/硬件动作，不访问用户 5433。资源关闭前其他连接和 schema 均为零；ignored 缓存保留。未推送/未建 PR，用户允许额度不足使用 reset 卡，本轮未使用。

{limits}
''')
for name in ['registry','http','regression']:
 p=h/(name+'-HANDOFF.md');t=p.read_text();t+=f'''\n## Root 最终验收补记\n\n上文“待 Root 验证”的状态为分工提交时刻；现已完成。源码 `{short}`，V2 manifest `{manifest}`。{summary} V1 原失败记录保留；详见 [Root HANDOFF](root-HANDOFF.md) 与 [证据索引](evidence-index.json)。任务资源已全部停止；本分工所有最终验收项已关闭。\n''';t=t.replace('- [ ] Root ', '- [x] Root ');p.write_text(t)
p=d/'resume-checkpoint-l12-2026-10-10.md';t=p.read_text();pos=t.index('\n\n');t=t[:pos]+ '\n\n本检查点为历史记录；Skills HTTP 后续清理已完成，最新从 [2026-10-11 检查点](resume-checkpoint-skills-2026-10-11.md) 继续。'+t[pos:];p.write_text(t)
p=d/'checklists/next-round.md';t=p.read_text();pos=t.index('\n## R01');t=t[:pos]+f'\n2026-10-11 Skills HTTP 索引清理 **10/10**，源码 `{short}`；六轮仍 **2/6**。最新恢复从 [Skills 检查点](../resume-checkpoint-skills-2026-10-11.md) 开始。三个 HTTP 索引缺口关闭，下一软件项为静态 classifier 候选与运行 Registry 的衔接审计。\n'+t[pos:];p.write_text(t)
p=d/'checklists/layering-next.md';t=p.read_text();pos=t.index('\n\n');t=t[:pos]+'\n\n后续更新：Skills HTTP CRUD reload 清理已完成（本轮 10/10），下一项软件审计见 [2026-10-11 检查点](../resume-checkpoint-skills-2026-10-11.md)。下方 L12 阶段的“下一项”保留历史语境。'+t[pos:];p.write_text(t)
p=d/'platform-validation.md';t=p.read_text();pos=t.index('\n\n');t=t[:pos]+f'\n\n**2026-10-11 Skills：10/10，L09—L12 4/4，六轮仍 2/6。** {identity} {summary} 当前仅完成此矩阵，未重跑 release、真实 PTY、部署 smoke 或硬件。资源全部关闭，见 [最新检查点](resume-checkpoint-skills-2026-10-11.md) 和 [最终验证](skills-2026-10-11.md#最终验证与证据)。以下为历史记录。'+t[pos:];p.write_text(t)
p=d/'deferred.md';t=p.read_text();lines=t.splitlines();out=[]
for line in lines:
 if line.startswith('- [ ] ') and ('Skills HTTP' in line or '三个 reload TODO' in line):
  line='- [x] 该 Skills HTTP 索引待办已在 2026-10-11 完成，见 [本轮检查点](resume-checkpoint-skills-2026-10-11.md)。三个入口统一变更 owner；无调用 plugin facade 已删除，实际 builtin/file 初始化保留。'
 out.append(line)
t='\n'.join(out)+'\n';t+=f'''\n## 2026-10-11：Skills HTTP 索引清理后的续接\n\n- [x] 本轮 10/10，源码 `{short}`，CRUD 索引准备/提交/发布、失败恢复、取消与 stop 所有权、旧 facade 清理及同源双平台回归完成；见 [最新检查点](resume-checkpoint-skills-2026-10-11.md)。\n- [ ] {nexttask}\n- [ ] 多实例／外部 SQL 的自动索引通知未实现；当前通过显式 reload。仅出现明确多实例需求时再立项，不为未来场景引入广播依赖。\n- [ ] 原 V01、R02/R03/R05/R06 与 D01—D05、C03—C05、H01—H05 保持原状态。默认 Shadow、工具可见性、执行批准和硬件边界保持。\n\n本轮未改变请求数组元素/null 语义；Skills 网络 COMMIT ACK 丢失没有实测注入，只有实现屏障、实际约束 COMMIT 失败/恢复回归和 RR 时序探针证据。没有新增平台、依赖或硬件动作。\n''';p.write_text(t)
print(json.dumps(dict(source_commit=commit,macos=mac,linux=linux,checklist='10/10'),ensure_ascii=False))
