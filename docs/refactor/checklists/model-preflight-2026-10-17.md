# 软件发布资料与真实模型链前置 checklist

2026-10-17；起点 `6a8b113`，受测应用源码 `6c62c84ee60e3a5fd387f2172ead432031907599`。本轮 **10/10：资料收口，用户答复及明确待定项已登记**。这里的计数不是模型验收通过率；R01—R06仍2/6。

- [x] MP01 读取现有配置、实现、上一轮四份交接及证据；复用三个subagent分工，不从示例推定实际部署。
- [x] MP02 核对起点678输入、两个Linux产物的字节数／摘要和148份既有归档，保留受测源码身份。
- [x] MP03 明确当前Linux全量／release／noop进程、当前Mac定向、旧Mac全量与历史Easy-Turn证据边界。
- [x] MP04 核对LLM角色／profile／provider、embedding身份、配置覆盖、SQL政策与认证来源契约。
- [x] MP05 核对Easy-Turn、Whisper、GPT-SoVITS的真实入口、模型身份、health与初始化副作用。
- [x] MP06 只修已确认资料缺口：旧`speech.enabled`、旧STT纠正文案、Memory启动健康语义；补齐[发布资料](../../operations/software-release.md)。
- [x] MP07 集中发出三组确认问题；把所有未知实际值明确登记为待确认，不收集密钥或自动访问端点。
- [x] MP08 固定下一轮组件→既有Easy-Turn live→真实LLM/TTS组合证据的顺序、原期限／并发和owner清理要求；不预建框架。
- [x] MP09 独立复核、静态差异／TOML语义／本轮链接核对，保存四份HANDOFF、原件索引、台账及恢复检查点，随本轮正常提交。
- [x] MP10 收到并登记三组答复：BotCF候选基址、模型／embedding待定；Developer模型目录、Juno与项目Whisper按旧配置核对；开发验证沿用当前账户、必要时容器。已盘点指定路径与旧配置，精确模型／参考素材和实际运行身份未验，保持待定。

## 静态验证与范围

本轮只删除`config/server.example.toml`两行注释，前后解析后的TOML对象完全一致。678个受测输入中677个仍逐字节匹配；唯一变化为上述注释。没有Rust、Cargo、依赖、测试或运行政策修改；旧manifest和产物保留其原身份，没有重新构建或运行测试。本轮只读证据见[索引](../handoffs/2026-10-17-model-preflight/evidence-index.json)。

默认shadow、probe=false、权限／执行批准、原测试期限／默认并发、单consumer、精确取消及资源清理保持。未启动或探测模型／外部服务／设备，未连接Pi。R03仍暂停，历史V01根因仍open；Mac当前完整发布矩阵、真实模型链、绑定恢复、crash／断电、现场R02／R05与长稳均未由资料收口替代。

## 下一轮执行 checklist（尚未开始）

- [ ] 读取[恢复检查点](../resume-checkpoint-model-preflight-2026-10-17.md)与四份交接，基于MP10答复补齐具体模型、协议、Juno权重组合与参考素材；缺失资产不自动下载。
- [ ] 明确首轮调用范围和资源归属，形成具体配置；分别核对base／`.d`／env／settings／SQL政策，避免触发未选择的真实provider或共享TTS切权重。
- [ ] 补当前Mac对应平台的软件发布证据；Linux ELF产物不能代替Mac release。
- [ ] 在确认范围内完成真实组件及既有Easy-Turn两目标；保留原失败和原期限，不扩大ignored范围。
- [ ] 完成实际Generation／Delivery的LLM→TTS组合证据；私有文件输出、不播放，核对原owner结束与资源回收。

继续时不得将上述待办默认当作全部获准执行的命令；模型具体选型／真实请求范围仍缺，当前请求只授权前置核对。开发验证不要求先建正式服务账户，复用当前账户和任务独立目录即可，必要时再容器隔离。完整范围见[验收交接](../handoffs/2026-10-17-model-preflight/acceptance-HANDOFF.md)。
