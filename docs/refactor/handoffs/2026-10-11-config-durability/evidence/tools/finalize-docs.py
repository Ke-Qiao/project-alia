from pathlib import Path
import json
r=Path(__file__).resolve().parent;root=r.parents[1];d=root/'docs/refactor';h=d/'handoffs/2026-10-11-config-durability'
v=json.loads((r/'commit-verification.json').read_text());s=json.loads((r/'final-stage-summary.json').read_text());c=json.loads((r/'resource-closure.json').read_text())
assert v['manifest_sha256']==s['manifest_sha256'] and v['input_count']==v['matching_git_blobs'] and not v['mismatches']
assert c['all_task_containers_stopped'] and c['other_database_connections']==0 and not c['remaining_test_schemas']
mac=s['stages']['macos/workspace-v3'];lin=s['stages']['linux/server-tests'];commit=v['source_commit'];sha=v['manifest_sha256'];n=v['input_count']
def counts(x):return '/'.join(str(x['counts'][k]) for k in ['passed','failed','ignored'])
identity=f'源码 `{commit}`，V3 manifest `{sha}`，{n}/{n} Git blob 一致。'
results=f'macOS workspace **{counts(mac)}**；Linux ARM64 Server 默认并行完整回归 **{counts(lin)}**。两端无默认 feature 全目标检查、架构边界正反自测、变更文件格式检查、计数器 8 例自测通过。数字为 passed/failed/ignored；嵌套子进程摘要不重复累计，最终阶段同源、无源码漂移。'
closure=f'全部 Cargo 阶段结束；关闭前其他数据库连接为零，最终测试 schema 为零，遗留 schema 清理数 {len(c["leftover_schemas_removed"])}。本轮 V1/V2/V3 Linux holder 与独立 PostgreSQL 已停止；用户 5433 未访问。空闲 holder 的停止码不作为 Cargo 测试结果。缓存、只读快照保留在 ignored `run/validation-20261011-config-durability/`。'
limits='未运行真实断电、独立进程 kill/restart、Linux 全 workspace、release/native release link、部署 smoke、真实 PTY、外部或付费模型、Pi/声学/执行器及长稳验证。故障注入与丢弃旧 setup owner 后加载证明软件恢复语义；不代表任意存储设备兑现 flush，也不提供其他本机进程的文件系统锁。'
next_task='下一软件候选：先审计 `SettingsRegistry::save` 与 `PATCH /api/v1/settings` 的 live apply、内存配置及 `settings.toml` 持久化顺序，定义保存失败与重试的恢复语义，再在授权范围补缺口。保持 API、当前政策和权限；先检查既有实现，不直接扩展本轮发布协议为通用事务框架。'
validation=f'''## 最终验证与证据

{identity}

{results}

| 命令 | 平台 | 结果 / 秒 |
| --- | --- | --- |
| `cargo test --workspace --offline --locked` | macOS ARM64 | {counts(mac)} / {mac['elapsed_seconds']} |
| `cargo test -p alia-server --offline --locked` | Linux ARM64 | {counts(lin)} / {lin['elapsed_seconds']} |
| `cargo check -p alia-server --no-default-features --all-targets --offline --locked` | macOS ARM64 | exit 0 / {s['stages']['macos/no-default-v3']['elapsed_seconds']} |
| 同上 | Linux ARM64 | exit 0 / {s['stages']['linux/server-no-default']['elapsed_seconds']} |

V1 Server lib 为 **357/0/3**（96.29 秒），随后主动复核发现早期失败遗留父目录的重试缺口。V2 删除祖先计数并新增针对回归，macOS workspace 为 1108/0/5。V2 Linux 第一次运行中途计数为 480/1/3，在既有 `control_ack_loss_is_observed_from_original_device_command` 的立即 `!is_held()` 断言失败（exit 101），同一二进制默认并行 9 项随后全部通过（13.445 秒）；这次复查不能抹去首次失败。代码顺序证明数据库 receipt 可见先于本地 gate 解除，V3 仅让该测试在共同的原 3 秒期限内等待两者，保留最终断言、原 32 秒场景期限和 cleanup，不改生产 Playback。最终验收采用 V3 两端完整回归。V1 Linux 仅记录环境；V2 Linux 无默认检查因前置失败没有执行。所有版本清单与原始日志保留，不能把部分运行累计为最终通过数字。

macOS 26.6.2 ARM64 / Debian 12 ARM64，Rust 1.95.0；仅复用已有离线依赖、native archive 与容器镜像。macOS 标准库目录 sync 的独立探针通过。定向阶段与全量包含重复用例，不相加作为独立测试数；5 个 ignored 仍按原环境门槛保留。

{closure}

原始命令、日志、冻结清单、Git 输入核对与资源状态见 [证据索引](handoffs/2026-10-11-config-durability/evidence-index.json)。

{limits}'''
p=d/'config-durability-2026-10-11.md';t=p.read_text();assert '最终运行数字、冻结身份与提交将在统一验收后补充。' in t;p.write_text(t.replace('最终运行数字、冻结身份与提交将在统一验收后补充。',validation))
p=d/'checklists/config-durability-2026-10-11.md';t=p.read_text().replace('V3 macOS 同源验收通过，当前 **7/10**；Linux、交接与提交待完成','本轮 **10/10 完成**').replace('- [ ]','- [x]');p.write_text(t+f'\n{identity}\n\n{results}\n\n{closure}\n\n{next_task}\n\n恢复入口：[本轮检查点](../resume-checkpoint-config-durability-2026-10-11.md)。\n')
for name in ['storage','flow','regression']:
 p=h/(name+'-HANDOFF.md');t=p.read_text();t=t.replace('- [ ] Root', '- [x] Root')
 t=t.replace('V2 完整父链修复尚待验证。','V2 完整父链修复现已纳入最终同源验收，见下文。').replace('上方验收与提交 **0/3**，最终结果由 Root 追加。','上方验收与提交现为 **3/3**，最终结果见 Root 补记。')
 t=t.replace('当前本分工 **6/8**。','实施交接时为 **6/8**，现经 Root 最终验收为 **8/8**。').replace('源码处于本轮实施状态，尚未编译或运行测试。','此处保留 subagent 实施交接时点：当时尚未编译或运行测试，Root 最终验收见下文。')
 t+='\n## Root 最终验收补记\n\n'+identity+'\n\n'+results+'\n\n'+closure+'\n\n上文 subagent 实施阶段的进度数字与未运行说明保留历史时点；对应 Root 验收事项已勾选，现已完成同源整合验收、资源关闭与源码提交，本轮总 checklist 为 10/10。\n\n'+limits+'\n';p.write_text(t)
(h/'root-HANDOFF.md').write_text(f'''# 配置发布与恢复 Root HANDOFF

承接 `7165ad6`，分支 `codex/architecture-refactor`。继续三个 subagent：`skills_registry` 负责配置域持久化与内部故障测试，`skills_http` 负责 Setup 完成 owner、HTTP/CLI 接线与实际 drain，`skills_regression` 负责真实 HTTP 与丢弃旧 owner 后 loader 恢复。Root 负责 discovery/load/needs_setup、交叉审阅、唯一 Cargo/PG/Docker 窗口和正常提交。三方已交接。V2 Linux 发现旧播放测试缺少本地 gate 完成见证，V3 仅修该测试等待条件，保留原 3 秒整体期限与生产逻辑；首次失败和同二进制复查均已存档。

本轮 **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**，不是项目整体百分比。先检查旧实现再补缺口：私有候选、原子发布和单赢家沿用；发布/恢复归配置域，删除 OOBE 旧写入口。完整行为见 [技术说明](../../config-durability-2026-10-11.md)。

{identity}

{results}

{closure}

## 恢复步骤

1. 读取 [检查点](../../resume-checkpoint-config-durability-2026-10-11.md)、本轮 checklist、其他三份 HANDOFF 和 [证据索引](evidence-index.json)，核对 branch/status/log。
2. 本轮无待恢复的实现或验证作业。新变更新建输入清单，不覆盖本轮原始日志与冻结记录。
3. {next_task}
4. 可信文件系统部署边界、V01、R02/R03、Pi/24h/R05、R06 和前端等暂缓项目继续见 [延期台账](../../deferred.md)，不自行扩大硬件/平台/外部服务范围。

未推送、未建 PR，本轮未使用 reset 卡。

{limits}
''')
(h/'README.md').write_text(f'''# 2026-10-11 配置持久化交接

本轮 **10/10**。{identity}

[Root](root-HANDOFF.md) · [Storage](storage-HANDOFF.md) · [Setup flow](flow-HANDOFF.md) · [HTTP/loader 回归](regression-HANDOFF.md)。

[技术说明](../../config-durability-2026-10-11.md) · [checklist](../../checklists/config-durability-2026-10-11.md) · [恢复检查点](../../resume-checkpoint-config-durability-2026-10-11.md) · [证据索引](evidence-index.json)。

{results}

{closure}
''')
(d/'resume-checkpoint-config-durability-2026-10-11.md').write_text(f'''# 当前检查点：2026-10-11 配置持久性与发布恢复完成

分支 `codex/architecture-refactor`；本轮 **10/10**，L09—L12 **4/4**，R01—R06 仍 **2/6**，不是项目整体百分比。三个 subagent 已交接，无待恢复 Cargo/PG 作业。

依次读取 [技术说明](config-durability-2026-10-11.md)、[checklist](checklists/config-durability-2026-10-11.md)、[四份 HANDOFF](handoffs/2026-10-11-config-durability/README.md)、[证据索引](handoffs/2026-10-11-config-durability/evidence-index.json)，然后核对 branch/status/log。

{identity}

发布/恢复归配置域；候选与 fragments、目录树和完整 canonical 祖先链同步；私有相邻 descriptor 记录同一意图。Setup 单一 watch 为 Idle/Pending/Ready/Cancelled，Pending 保留文件，仅同路径/内容/安装方式可恢复，取消封门但不删除记录。主文件未发布时启动也识别 pending；正常无记录 load 仍只读。保留 API、当前政策、来源限制和权限，不增加依赖或平台。

{results}

{closure}

**{next_task}**

可信文件系统服务账户与路径部署、真实断电、V01、R02 AEC、R03 反馈、Pi/24h/R05、R06 和前端等暂缓项仍开放，见 [台账](deferred.md)。自定义 setup 目标重启须继续指定相同 `ALIA_CONFIG_PATH`；恢复检测到改动/丢失时保留现场并报错，不能仅删除记录强行继续。

未推送、未建 PR，本轮未使用 reset 卡。{limits}
''')
p=d/'resume-checkpoint-health-2026-10-11.md';lines=p.read_text().splitlines();lines[2:2]=['后续更新：配置目录持久性与发布恢复已收口，最新恢复从 [配置检查点](resume-checkpoint-config-durability-2026-10-11.md) 开始；以下保留 Health 轮历史状态。',''];p.write_text('\n'.join(lines)+'\n')
p=d/'checklists/layering-next.md';t=p.read_text();a=t.index('后续更新：');b=t.index('\n',a);p.write_text(t[:a]+'后续更新：配置目录持久性与发布恢复已完成 10/10，最新恢复见 [配置检查点](../resume-checkpoint-config-durability-2026-10-11.md)。下方旧阶段的“下一项”保留历史语境。'+t[b:])
p=d/'checklists/next-round.md';t=p.read_text();anchor='## R01 ·';assert anchor in t;p.write_text(t.replace(anchor,f'2026-10-11 配置目录持久性与发布恢复 **10/10**，源码 `{commit[:7]}`；六轮仍 **2/6**。最新恢复从 [配置检查点](../resume-checkpoint-config-durability-2026-10-11.md) 开始。{next_task}\n\n'+anchor))
p=d/'checklists/management.md';t=p.read_text().replace('health 续轮已收口','health 与配置持久性续轮已收口');old='- [ ] **配置目录持久性**：目前 base 文件 sync+原子发布，不同步父目录/新 fragments；需要定义发布后 sync 失败的未知结果、目录树/祖先同步及恢复规则，再在隔离文件系统验证。不能把进程 kill 当真断电。';assert old in t;t=t.replace(old,f'- [x] **配置目录持久性软件闭环**：源码 `{commit[:7]}`，base/fragments/目录树与完整 canonical 父链同步，未知发布保留文件与 descriptor，同意图重试及 loader 恢复；正常加载保持只读。隔离文件系统双平台验证见 [本轮 10/10](config-durability-2026-10-11.md)。真实断电与部署文件系统保证不由软件测试代替。').replace('其中 health 已在 2026-10-11 单独验收，其余两项继续开放','其中 health 与配置持久性软件闭环已在 2026-10-11 分别验收，可信本机部署边界继续开放');p.write_text(t)
p=d/'deferred.md';t=p.read_text().replace('配置目录断电持久性和可信本机文件系统部署边界仍开放','配置发布/恢复软件闭环已由 [配置轮](resume-checkpoint-config-durability-2026-10-11.md) 验收，真实断电与可信本机文件系统部署边界仍开放');old='- [ ] 下一软件任务：配置目录持久性。先核对 base 文件与 fragments 的创建、同步和原子发布调用链，定义发布后父目录 sync 失败的未知结果、目录树/祖先同步和恢复规则，再补必要实现与隔离文件系统回归。保持当前 API、政策、权限与已有依赖；真实断电不能用进程 kill 代替。本轮未实施这项。';assert old in t;t=t.replace(old,'- [x] 后续已由 [配置轮](resume-checkpoint-config-durability-2026-10-11.md) 完成目录同步与发布恢复软件闭环；原 Health 轮未实施，历史 scope 保留。');t+=f'\n## 2026-10-11 配置持久性清理后的续接\n\n- [x] 本轮 10/10，源码 `{commit[:7]}`，保持 API、政策、权限与既有依赖；发布归配置域、同意图恢复、四份交接及同源双平台验收已闭环。\n- [ ] {next_task}\n- [ ] 真实断电、部署账户/目录权限及同机其他进程信任仍需实际部署核对。恢复记录建立前的 crash 可留候选或 CLI fragment tree；无法确认所有权的文件不自动删除。\n\n{limits}\n';p.write_text(t)
p=d/'platform-validation.md';lines=p.read_text().splitlines();lines[2:2]=[f'**2026-10-11 配置持久性：10/10，L09—L12 4/4，六轮仍 2/6。** {identity} {results} 见 [本轮技术说明](config-durability-2026-10-11.md#最终验证与证据)。{limits} 以下旧结果保留各自源码语境。',''];p.write_text('\n'.join(lines)+'\n')
print(json.dumps(dict(source_commit=commit,manifest=sha,mac=counts(mac),linux=counts(lin))))
