# Root 交接：L09 固定核心与契约归属

基线 `b9ff6e0`；分支 `codex/architecture-refactor`。源码提交 `ffd7af19dccb8db28f16c5091e09265d4c5bbd32`，本轮 checklist **10/10**；最终验收与证据归档已完成。

## 已完成的整合

三个 subagent 按独占文件分工，Root 统一迁移 providers 共享契约、Bootstrap 装配、ModuleRegistry / ModuleContext、API、Channel、Interaction、Companion、工具、scheduler 和内外部测试。生产装配持具体服务；ConversationStore / CacheStore 仅保留实际消费与故障注入需要的窄业务接口，移除 Lifecycle 继承和无意义 mock lifecycle。

Router 在 minor LLM 初始化后由原配置显式构造，再按原资源阶段纳入 cleanup；无未初始化 Router 状态或 DirectRouter 测试替换实现。删除 RouteDecision 中无生产构造者、无规范缓存身份的 CacheHit；实际缓存继续规范 Response 完成路径。Context capture 关联函数直接接收数据库，移除 Memory maintenance 四处无效 ContextService / NoopEmbedding 构造，原事务体不变。

原七个固定 provider 目录、旧 exports、旧 trait/实现名及五个单实现工厂删除。架构守卫覆盖旧路径、旧名、固定工厂、反向重新导出与 canonical owner 缺失；合理域端口及外部 adapter 不误报。

## 验证窗口与源码身份

Root 独占 Cargo / PostgreSQL。V1 manifest `2a06adad2d01d3faee0d369576a907af61d23918f18e5d956a05556dcf0923e5`，637 输入，全部匹配源码提交 Git blob。默认测试目标与 no-default all-targets 整合编译已通过；第一次默认编译因测试文件未导入 ConversationStore 失败，原日志单独保留。

本轮任务 PG `alia-l09-20261009-db`，动态回环端口 32771，数据库 `alia_l09_20261009`。用户 5433 不访问。Linux holder `alia-l09-linux-20261009-v1`，使用已有固定镜像、只读 V1 快照，以及已停止上轮任务的缓存；无下载。常规构建 jobs=2，测试默认并发/栈；PTY 单线程真实终端。默认 Shadow、现行政策与无真实模型/硬件动作边界保持。

同源 macOS workspace / PTY / no-default、Linux ARM64 Server 全包 / no-default / release-link 均已完成，见末尾 Root 最终验收。

## 下一步与未完成边界

本轮验证完成且任务资源已关闭，从 L10 初始化依赖收窄继续；L11 跨域事务原子性、L12 出站契约为独立项。Skills API 的既有三个 CRUD reload TODO 尚未解决，应独立沿实际调用链补齐，不把本轮插件 hot reload 保留误作该功能已完成。

前端、真实 AEC/R02、执行器反馈/R03、V01、Pi/24h/R05、其他指定平台/R06、端上模型继续见延期台账。用户允许额度不足时使用 reset 卡，但本轮尚未使用，也没有以该授权代替恢复文档。

## Root 最终验收

源码 `ffd7af19dccb8db28f16c5091e09265d4c5bbd32`；V1 `2a06adad2d01d3faee0d369576a907af61d23918f18e5d956a05556dcf0923e5`，**637/637** 输入匹配 Git blob。macOS workspace **1056/0/5**、真实 PTY **3/0**；无默认 Insights availability **2/0**；Linux ARM64 Server 全包 **774/0/5**、默认 release/native link 与无默认检查通过。macOS 无默认 all-targets、fmt 和边界正负自检通过。

本轮 **10/10**，L09—L12 **1/4**，六轮 **2/6**。实际命令、计数和限制见 [最终验证](../../layering-2026-10-09.md#最终验证与证据) 与 [证据索引](evidence-index.json)。本轮任务资源已停止，三个 subagent 已交回，没有待接回的 Cargo/PG 作业；下一步 L10。两项真实模型未重跑，Linux 非全工作区及独立部署 smoke，不声称硬件验收。
