# 软件发布资料与真实模型链前置 — Root HANDOFF

2026-10-17；本轮起点`6a8b113`，受测应用源码`6c62c84ee60e3a5fd387f2172ead432031907599`。本轮 **10/10**：资料与只读审计完成，用户答复及明确未定项已登记；真实模型验收未执行。

## 分工与交付

| Owner | 范围 | 结果 |
| --- | --- | --- |
| `v01_fixture` | 配置、role/profile/provider、认证、embedding、语音服务与初始化副作用 | [config-HANDOFF](config-HANDOFF.md)，6/6审计完成 |
| `v01_flow` | 已有真实Easy-Turn与LLM/TTS入口、样本、原期限／并发和后续顺序 | [acceptance-HANDOFF](acceptance-HANDOFF.md)，6/6前置完成，未执行模型 |
| `v01_review` | 源码／产物／旧证据身份、平台结论和资料交叉复核 | [review-HANDOFF](review-HANDOFF.md)，独立只读复核 |
| Root | 集中确认、发布记录、修正文案、静态验证、台账／checklist／恢复检查点与正常提交 | 本文件及[发布记录](../../../operations/software-release.md)；MP10答复已登记 |

## 已确认缺口及修改

`docs/operations/model-services.md`删除旧`speech.enabled`启用指引，明确模块选择／VoiceSession、role与profile、embedding、配置覆盖、默认不探测与真实输出分离、TTS远端权重切换及loaded identity、既有live测试范围。Memory说明更正为Healthy/Degraded继续且保持enabled，Unhealthy重试耗尽导致启动失败，不静默关闭Memory。

`config/server.example.toml`仅删两行旧STT纠正角色注释，配置值／TOML解析对象完全相同。当前VoiceSession直接使用Easy-Turn transcript，不预建第二STT／纠正入口。生产代码、依赖和测试不变，确认的缺口都在资料层。

新增[软件发布记录](../../../operations/software-release.md)，包含受测源码、Linux产物摘要、平台／进程证据边界、配置／恢复入口及有界验收顺序。用户已确认BotCF候选测试基址、模型／embedding待定；指定Developer内Easy-Turn/GPT-SoVITS和Juno权重、项目Whisper，并要求参考旧配置。两agent随后只读盘点；base/fragments与persisted settings含不同历史模型／provider值，不直接合成新方案。开发验证使用当前账户／任务目录，必要时容器隔离，正式服务账户不作前置硬门槛。`default`若不等于minor实际取major而不是独立第三profile，这一既有差异只作边界登记。

## 验证与可复查证据

- 起点只读检查678输入全匹配旧manifest、两个独立release副本hash及148份旧归档一致。见[baseline](evidence/baseline-identity.json)。旧失败／原INVALID／派生行映射不改动。
- 当前静态检查：677输入仍相同，1项仅注释变化；与起点及受测提交分别解析TOML后相同；实际Linux产物hash不变。新资料相对链接可解析，diff无空白错误；见[静态核对](evidence/static-verification.json)和[索引](evidence-index.json)。
- 当前Linux全量1287/0/5、双端release／链接和双轮noop进程恢复是**上轮**结果；当前Mac只有2项定向，完整1277/0/5属于旧源码。Linux产物不能当作Mac发布，正常noop重启不能当作绑定／崩溃恢复。
- 本轮没有Cargo／live／模型调用、端点探测、服务创建或硬件操作，因此没有新增运行结果或性能数字，不声称通过真实LLM/TTS链。
- 用户答复后的指定目录盘点找到Easy-Turn钉住checkout与checkpoint、SHA匹配的三份官方PCM、GPT-SoVITS现有CPU/v2ProPlus Juno两权重；Whisper实际位于项目安装脚本默认`.local/share/alia/whisper.cpp`并有turbo文件。没有重hash大权重或加载模型；Juno参考WAV／文本仍未选。GPT旧脚本默认全部接口，后续任务启动显式loopback；原模型服务配置未改。

## 恢复工作与限制

从[当前检查点](../../resume-checkpoint-model-preflight-2026-10-17.md)继续：MP10资料答复已登记，精确聊天／embedding模型、TTS权重组合与参考素材／运行身份仍需补齐；不要重复索问已经确定的Mac目标或开发部署方式。只索取受控路径和变量名，不接收密钥；缺失资产不自动下载。之后在确认范围内逐组件验证、复用既有Easy-Turn两个live目标，再补真正Generation/Delivery的LLM→TTS组合与原owner清理。保存文件，不播放。

真实模型服务可能有共享权重切换或外部请求副作用，必须按已明确范围进行；端点资料本身不是启动授权。保持默认shadow／probe=false、当前政策、原测试期限／并发和执行批准，不用健康成功代替infer身份。R03暂停，R02资料／R05现场、Mac部署／长稳未验；历史V01仍open，六轮仍2/6。
