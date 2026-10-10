# Classifier 与运行技能目录衔接

承接 `4e4d595` 的下一软件任务，分支 `codex/architecture-refactor`。本轮先审计既有实现，再删除静态技能表、专名捷径与无目录约束的技能名接受路径。默认 Shadow、路由模式、全局工具可见性和 Execution 批准保持。无新增依赖、feature、平台、模型或硬件动作。

## 实际缺口与归属

HTTP CRUD 已保证 SQL 提交后发布完整 Registry，但旧 classifier 提示仍固定列出七个技能，Router 接受任意非空技能名；Generation 另行读取技能上下文，可能在分类等待期间混用两个版本。exact / semantic 缓存还能绕过分类，且没有技能目录绑定。

现在 Registry 在一次短读锁内生成 `SkillCatalog`：候选只含 name、display_name、description、category、keywords；完整定义摘要保留在服务内部。候选按 priority 降序、name 升序排列。Router 只消费此次实际候选，不读取数据库或 Registry、不解析可执行提示；返回未知名称按原模式回退。major-only 仍跳过 classifier，minor-only 不升级，both 仍回退 major；confidence 仍用于诊断。

Interaction 每轮捕获一次 catalog，用于展示列表、分类候选和缓存指纹。模型返回技能后，Registry 在同一个读锁内核对原 ID 与完整内容摘要，再返回实际上下文。选中项被更新、禁用、删除或同名重建时返回缺席，Interaction 依据当前模式正常回退，无二次分类。其他技能改变不会废弃未变的选择。preferred tools 继续只影响排序，不能创造工具权限。

定义摘要覆盖 ID、展示元数据、keywords、prompt、cacheable、priority 和排序后的 preferred tools；enabled 体现在成员集合。时间戳不参与摘要，普通 reload 和仅更新时间戳不改变身份。全目录指纹由排序后的定义摘要组成，沿用已有 SHA-256 与 serde_json。

## 缓存与并发边界

exact、semantic direct 和 semantic reference 同时要求原有批准/来源/隐私/到期条件、相同目录指纹及当前目录检查。旧条目没有指纹时不复用；新建技能也会改变目录，因此以前的通用答案不能跳过新候选。缓存审核只能保留原绑定，不能通过 review metadata 把旧答案重贴为新目录。

生成结束仅在原目录仍当前时安排写入，并保留 pending review。Registry 初始不可用时不提供技能与缓存上下文；结束时的可选写入检查失败则不写缓存。已有缓存读取/解析错误路径保持。目录、解析和三处缓存目录检查均通过 `cancellable_stage`，取消等待锁会走既有 interrupted 路径，保留用户来源与终止审计，不安排 assistant 持久化。

线性化点：选择在 `resolve` 成功处生效；缓存消费在 `is_current` 成功处准入；缓存写入资格在 finish 检查时确定。随后并发 CRUD 不撤销已经准入的生成或文本。已排队缓存仍携原指纹，未来新目录无法命中它。这不是“直到最终数据库持久化都必须保持目录不变”的保证。跨实例/外部 SQL 仍须显式 reload。

## 验证与交接

源码 `744cae2a11f8192f0e8ccf3d13004402202c1411`，V2 manifest `adeeaa0d150e6c7a036beeb05aa409da1d811e130d60354fe46e290c2ec132b5`，644/644 Git blob 一致。

macOS workspace **1080/0/5**；Linux ARM64 Server 默认并行完整回归 **798/0/5**。两端无默认 feature 全目标检查、边界正反自测和变更文件格式检查全部通过。计数依次为 passed/failed/ignored，嵌套子进程摘要不重复累计；所有最终阶段同源、无源码漂移。

V1 仅用于静态审阅，未编译、未运行测试；其中私有导入、三处目录锁取消遗漏及 major-only 测试替身接线问题在运行前修正，保留 V1 manifest 与审阅记录。

新增两条 HTTP→EventBus→Interaction 回归包含 5 轮 CRUD/未知名和 6 轮分类期间变更；同名重建保留相同内容但改变 ID。缓存回归覆盖已批准 exact/semantic 旧目录拒用及审核绑定不被替换/补造。真实目录写锁的取消定向证据覆盖 finish 阶段；exact/semantic 的目录等待包装通过代码审阅，既有回归继续覆盖这两条缓存读取的取消出口，不将其描述为三个阶段都做了相同的锁注入。

入口：[checklist](checklists/classifier-2026-10-11.md)、[四份 HANDOFF](handoffs/2026-10-11-classifier/README.md)、[恢复检查点](resume-checkpoint-classifier-2026-10-11.md)、[证据索引](handoffs/2026-10-11-classifier/evidence-index.json)。

本轮不证明真实模型分类质量、硬件音频/AEC、执行器反馈、Pi 或 24 小时负载。原 V01 与 R02/R03/R05/R06 继续开放；L09—L12 保持 4/4，R01—R06 保持 2/6，不换算项目完成百分比。

## 最终验证与证据

| 平台／检查 | 命令 | 结果 | 耗时 |
| --- | --- | --- | --- |
| macOS 默认 workspace | `cargo test --workspace --locked --offline --no-fail-fast` | 1080/0/5 | 526.8 秒 |
| macOS 无默认全目标 | `cargo check --workspace --all-targets --no-default-features --locked --offline` | exit 0 | 46.35 秒 |
| Linux ARM64 默认 Server | `cargo test -p alia-server --locked --offline --no-fail-fast` | 798/0/5 | 388.073 秒 |
| Linux ARM64 无默认全目标 | `cargo check -p alia-server --all-targets --no-default-features --locked --offline` | exit 0 | 16.386 秒 |

macOS 26.6.2 ARM64 与 Debian 12 ARM64 容器，Rust/Cargo 1.95.0；两端 CARGO_BUILD_JOBS=2，RUST_TEST_THREADS/RUST_MIN_STACK/DOCS_RS 未设置，沿用原生归档和离线缓存。5 项忽略为 3 项真实 PTY 与 2 项真实 Easy-Turn 音频验证，未静默启用或计为通过。V2 首次完整运行两端均通过，没有延长期限或降低并行度。

所有 Cargo 阶段结束；关闭前其他数据库连接与测试 schema 均为零，任务 Linux holder 与 PostgreSQL 已停止，用户 5433 未访问。空闲 holder 的 sleep 进程在 docker stop 后 exit 137，验证命令均 exit 0；未把容器停止码当作测试结果。缓存与只读快照保留在 ignored `run/validation-20261011-classifier/`。

下一软件任务：从已记录的 R04 整体 health 限额开始，审计 `api/health.rs`、ModuleRegistry 顺序聚合、Router 重复 minor probe 及 Provider 诊断；先检查现有实现，再收敛整体预算、同 profile 重复探测和并发边界。保持 API、默认 probe=false 和当前政策，不自行启用外部或付费探测。本轮未实施。

未运行 Linux 全 workspace、release/native release link、真实 PTY、部署 smoke、Skills 网络 COMMIT ACK 丢失注入、真实模型或硬件验证。目录写锁取消注入覆盖 finish 阶段；exact/semantic 目录等待未分别重复同样锁注入，已审阅包装并运行既有缓存取消出口回归。多实例／外部 SQL 仍需显式 reload。
