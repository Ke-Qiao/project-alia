# Root：L10 初始化依赖收窄完成

基线 `cb19992`，分支 `codex/architecture-refactor`。三个 subagent 完成各自实施和静态审查，Root 完成全局装配、测试、证据和资源关闭。

ModuleContext 与三个固定工厂已删除；组件显式绑定真实依赖，init 无参数，rollback slot 在任何初始化副作用前登记。Builder、factory、动态 Channel API、外部测试和 fixtures 采用同一新契约。独立审查发现的 Insights feature 准入后移已恢复原早期检查，新增提前失败回归通过。完整依赖表及后续入口见 [技术说明](../../layering-2026-10-10.md)。

Probe 的失败启动检查通过唯一 application_name 和参数化 pg_stat_activity 查询实际 owned pool 的连接存续与释放；成功构建的 native-stop 测试保留实际 pool 的 SELECT 1 与最终关闭断言。没有为测试增加生产注入 API。

Root 独占 Cargo/PG；常规 build jobs=2、默认测试线程和栈，真实 PTY 单线程。任务 PG `alia-l10-20261010-db` 使用 32768，Linux V2 holder 为 `alia-l10-linux-20261010-v2`，V1 holder 仅准备快照、未运行 Linux 验证；两者现均停止。日志、工具、失败记录和校验清单均在 evidence 内。

## Root 最终验收

源码 `4519e757542f644e9eb385d4055551546bf7abd3`；最终 V2 `d361dad3057717c4efb08b62ef8d98084754b276e420d0c2f842923a0e0f3e41`，**637/637** 输入匹配 Git blob。macOS workspace **1058/0/5**、真实 PTY **3/0**、无默认 Insights availability **2/0**、新增提前拒绝回归 **1/0**；Linux ARM64 默认 Server 全包 **776/0/5**。两端 Server 无默认 all-targets、Linux release/native link、fmt 和边界正负自检均通过。所有最终运行均无源码漂移。

本轮 **10/10**，L09—L12 **2/4**，R01—R06 仍 **2/6**。专用 PostgreSQL 和两个 Linux holder 已停止，数据库零残留测试 schema、零其他连接；无待接回的 Cargo/PG 作业。V1 无 feature 回归的报告包装断言失败已原样保留；V2 仅改这条测试断言，636 个输入及全部生产源码与 V1 相同。详见 [最终验证](../../layering-2026-10-10.md#最终验证与证据) 与 [证据索引](evidence-index.json)。

下一轮从 L11 跨域事务所有者开始，L12 和 Skills reload TODO 保留。真实模型、Pi/声学/机械、Linux 全工作区与部署 smoke 未在本轮验证；用户 5433 未访问，默认 Shadow 和政策保持。未推送或创建 PR，未使用 reset 卡。
