# Jev 对话决策接入

Jev 只替换意图分类。现有 VoiceSession／Easy-Turn 只有产生可提交的 Respond 后才进入生成；WAIT、未完成片段和取消不为路由另发请求。生成仍先验证会话与缓存，再决策，再复核技能定义，最后进入原回答／工具／Execution 路径。没有新增 STT、路由总线或执行入口。

## 角色与额外请求

`llm.roles.routing` 独立选择 `kind = "decision"` 的 profile；默认 `legacy` 保持旧 minor 分类器。启用 Jev 后，每次未被缓存直接回答的路由只请求一次 Jev；成功或失败都不回调旧分类器。

minor 仍负责日常回答、确实超出能力时的回答阶段 ESCALATE、按现有阈值触发的工具候选筛选，以及未配置 memory_capture 角色时的记忆提取。这些是独立请求，不计入分类提速。本轮没有换 minor／major 模型或改变其原期限、工具和授权流程。minor 的回答提示允许近期上下文追问与简单偏好，不再单凭这些因素升级。

Jev 绑定已有 minor／major 聊天服务，在每次决策的原期限内只读取当前 `model` 与 `max_output_tokens` 两项白名单值。当前两者同为 `deepseek/deepseek-flash`，输出预算分别为 1024／4096，不能用角色名称暗示它们是两个不同模型。state 明确说明这是配置与任务分工，不是实测质量；不会序列化整个 profile、provider、provider_specific 或密钥。聊天模型和输出预算通过原 Settings 入口热更新后，下一次决策会读取更新后的值；noop 或未配置 profile 的注入模型不伪造身份，也不为获取身份额外请求聊天模型。

Jev 的一次 choice 包含 `minor_llm`、`major_llm`、`clarify` 和本次有效技能索引。选项按可用角色收窄；技能描述仅作为 state 数据，私有执行 prompt 不发给分类器。`clarify` 映射到现有聊天路线，但不给这轮任何工具，也不进行 ESCALATE；运行期间模型角色开关变化仍保留此限制。技能返回后继续比较原目录中的持久身份与定义摘要，不能靠同名技能继承旧选择。

## OpenRouter 配置与身份

使用 OpenRouter 的专用 Decisions 协议，不能使用 Chat Completions 或 `typesafe/jev-router` 来代替 Alia 的路线判断。[官方 System One 兼容入口](https://openrouter.ai/docs/guides/community/typesafe-sdk)与 [Decisions API](https://openrouter.ai/docs/api/api-reference/alphadecisions/submit-a-decisions-request)接收 state／questions，不是聊天 messages／tools。模型目录需使用 `GET /api/v1/models?output_modalities=decisions`，普通目录默认列出 text 模型。当前已查询的决策目录把 `typesafe/jev-1.14` 对应为 `typesafe/jev-1.14-20261014`。前者是请求身份，后者是必须精确匹配的返回身份；不接受 latest／preview。版本对应变化会被拒绝并留下降级记录，不静默采用新模型。

```toml
[llm.roles]
routing = "routing_jev"

[providers.llm.openrouter_decision]
type = "typesafe"
base_url = "https://openrouter.ai/api/v1/systemone"
api_key = ""
api_key_env = "OPENROUTER_API_KEY"

[profiles.llm.routing_jev]
kind = "decision"
provider = "openrouter_decision"
model = "typesafe/jev-1.14"

[profiles.llm.routing_jev.provider_specific]
expected_model = "typesafe/jev-1.14-20261014"
```

`type = "typesafe"` 表示决策协议，不决定密钥供应商。`api_key` 为空时，工厂创建客户端才读取该 provider 明确指定的环境变量，不借用其他 provider 的 key；非空 `api_key` 表示用户显式提供的固定值，并优先于环境变量。环境变量取得的值不写入配置对象、Settings 或配置文件，因此下次创建客户端可以使用已轮换的值。上面的 OpenRouter 配置使用 OpenRouter 域名，不能把该 key 配给 TypeSafe 官方入口。不要把真实密钥加入示例、命令行参数、提交或日志。

客户端复用固定 Rig revision `c2ff3e30e6b8d84fc6759eab030656a42cae14ca` 的 `rig-typesafeai 0.44.0` 与既有 HTTP transport，没有升级其他 Rig 包。OpenRouter 返回数值 `usage.cost`，该版本 Rig 使用 Cost 对象；这里只把这个元数据字段转为 `{ "total": 原数值 }`，答案字节不改写，也不归一化概率。响应 question ID、选项集合、数值范围／分布、选择与最大概率关系继续由库验证；Alia 在 map 反序列化前另外拒绝重复概率键和超过 255 个概率选项，防止后写值掩盖非法值，并检查精确模型身份及必要的 input／output token 字段。禁止自动重试和 HTTP 重定向；endpoint 只接受无用户名、密码、query、fragment 的 HTTPS 或 loopback HTTP。

## 期限、取消与记录

沿用原 routing.timeout_sec（当前5秒），一次绝对截止包含请求、读响应和解析。失败只计算现有本地 fallback，不再开启另一段分类期限；随后的回答阶段仍服从其原有期限和取消规则。取消通过原 cancellable_stage 优先生效并丢弃请求future，不生成新回答或派发工具；本地取消不代表已确认供应商停止计算。

现有 `route_decision` 事件按原turn_id记录配置身份、实际返回身份（成功时）、规则版本、分类耗时、原始选项／概率／置信度、最终路线、澄清标记与有限降级原因。技能失效后分别保留模型选择和最终代码路线；取消时最终路线为空。回答完成后的 `route_execution` 记录真正承担回答的路线，包括原有minor→major升级。没有虚构模型理由，也不无条件记录用户文本、历史或供应商错误正文。失败无法取得实际身份时保留空值，不能把配置身份当作实测返回身份。

## 隔离验证、启用与恢复

本轮隔离配置在 `run/jev-routing-20261104/comparison/server.isolated.toml`：无channel、无discovery、不启动Server，数据库指向不可达的隔离占位地址，STT／TTS为noop，Easy-Turn关闭；诊断只构造两个分类客户端。比较使用预先标注的合成中文和同样的近期上下文，从已经准备好的RouterInput开始计时，不把上下文准备或历史SQL耗时算作旧分类模型耗时。真实接口逐样本每臂一次，无预热、无重试，原始失败保留；脚本假服务只能证明控制分支，不能证明语义正确。

这组预先标注的十条样本只测分类，不生成回答或执行技能：

| 样本 | 预期路线 | 判断依据 |
| --- | --- | --- |
| 问候 | minor | 直接日常回应 |
| “除了这些，你还能帮我做什么？” | minor | 读取近期能力介绍即可回答 |
| 一句话比较三个故障假设 | major | 答案长度不降低分析难度 |
| 无已知指代的“打开它” | minor＋clarify | 技能目录里只有一盏灯也不等于用户已指明它 |
| 简洁回答、少用术语的偏好 | minor | 简单偏好不是升级理由 |
| 多约束复习计划与两种策略比较 | major | 多约束规划；细节不足不能掩盖其复杂分析任务 |
| 明确打开书桌灯 | skill／desk_light | 本次目录中存在对应技能 |
| 近期已明确书桌灯后说“打开它” | skill／desk_light | 对象由上下文确定 |
| 改写引号内的越权指令 | minor | 引号内容是材料，不是执行命令 |
| 书桌灯技能描述夹带越权指令 | skill／desk_light | 保留明确技能匹配，忽略描述里的指令 |

`clarify` 的匹配要求同时满足 minor 路线与本轮无工具澄清标记；旧分类器只选 minor 不会被算作已有这一保证，这也不等于已经测过旧模型会怎样回答。技能样本只说明选择了目录中的处理路径，不能据此宣布授权或执行成功。超时、非法响应、取消及决策后的目录变化由本地可控测试覆盖，不靠向真实接口反复发坏请求制造通过记录。

诊断每条样本交替两臂先后顺序，各调用一次生产分类入口，分别保存 `request.json`／`result.json` 并拒绝覆盖旧文件。首次 Jev 后端失败将停止剩余 Jev 请求并逐条记为跳过；模型误选与后端错误降级分开记录。分类调用次数是诊断实际调用次数，不冒充额外抓包测得的 HTTP 次数；它不包含未执行的记忆提取、工具筛选或回答请求。原始材料在 [comparison 目录](../../run/jev-routing-20261104/comparison/)，结果见下节。

生产当前仍是Mac `d7126a9`、Pi `588c434`，本轮未切换。正式启用前应从已部署Mac源码形成只含本次增量的独立候选；主线尚含受V05限制的V03恢复改动，不能直接把主线二进制装到生产。检查候选后，将上面的provider/profile并入实际配置、把私有凭据来源接入现有Mac启动器，并设置 `core.llm.roles.routing = "routing_jev"`；所有新增身份设置都要求受控重启Server，当前请求不热切客户端。Pi无需为路由变更换固件或重新初始化。

恢复旧分类器：将有效 `core.llm.roles.routing` 持久化为 `legacy` 并受控重启Server；若从未通过Settings保存，则修改base配置中的 `llm.roles.routing`。如果已有Settings覆盖，不能只改base文件。`legacy` 不要求Jev密钥，也不会请求Jev；minor／major原角色继续工作。启用和恢复只涉及路由配置，不迁移或回滚数据库，不清空聊天、缓存、记忆或R1记录。

## 本轮实际结果

10 条事先标注的合成中文，每臂每条一次、交替先后、无预热或重试，原 5 秒不变。Jev 10 条均返回固定身份 `typesafe/jev-1.14-20261014`，全部符合标注，未发生接口失败、超时或代码降级。旧分类器 6 条符合，4 条过度升级 major：能力追问、对象不明的“打开它”、简单表达偏好、改写引号内指令。旧分流并未在本轮实际生成回答或执行工具，不能推断其回答一定怎样。

| 指标（包括首条） | 旧 minor 分类器 | Jev |
| --- | ---: | ---: |
| 样本／分类调用 | 10／10 | 10／10 |
| 符合预先标注 | 6 | 10 |
| 接口失败／超时 | 0／0 | 0／0 |
| 平均分类耗时 | 749.407 ms | 471.873 ms |
| 中位分类耗时 | 808.297 ms | 427.697 ms |
| 最小／最大 | 428.881／884.259 ms | 378.708／951.941 ms |

同输入逐对平均少等 **277.534 ms**，中位数减少 **47.09%**；9 对更快，首条问候反而从 668.540 增至 951.941 ms，全部纳入统计。不能把此结果写成“说完到开始播放快了47%”：本轮只测准备好的输入进入分类到分类结束，没有上下文／SQL准备、回答生成、工具筛选、记忆提取、TTS或播放。新路线为普通 minor 4条、minor 无工具澄清1条、major 2条、技能3条，并未把所有请求压到 minor。每轮仍是一次分类请求，只是把旧 minor 分类替换成 Jev；回答、工具筛选和记忆请求没有被此数字抵消。

[逐对结果与统计](../../run/jev-routing-20261104/comparison/analysis.json)、[全部原始请求／结果](../../run/jev-routing-20261104/comparison/live-01/)、[诊断输出](../../run/jev-routing-20261104/comparison/live-01.log)保留。每条结果包含合成 turn／response ID、规则版本、返回身份、原始选项／概率、最终路线和降级状态。比较程序 exit 1 表示旧分类器有4条不符合标注，不是API失败；没有随后重跑改写结果。真实凭据仅在内存进入子进程环境，未进入配置、参数、日志或提交。

软件验证在 macOS ARM64、Rust 1.96.0、真实独立 PostgreSQL 17.10／pgvector 下运行，保留默认并发和原期限。先完成下表八个目标；随后只因无默认依赖修复和聊天配置动态读取补丁复验受影响目标，分轮统计，不相加。

| 定向目标 | 首次完整集合 validation-v2（通过／失败／忽略） | 最终影响复验 validation-v4 |
| --- | ---: | ---: |
| 路由／协议单元 | 18／0／0 | 19／0／0（新增未知聊天身份用例） |
| 旧 Router 相容 | 15／0／0 | 无相关后续改动，未重复 |
| Jev 错误／截止／策略变化 | 4／0／0 | 无相关后续改动，未重复 |
| 配置与凭据 | 11／0／0 | 11／0／0 |
| Settings 保存与恢复 | 7／0／0 | 无相关后续改动，未重复 |
| LLM provider／factory | 6／0／0 | 6／0／0 |
| 真实PG运行时分类、目录变化与取消 | 5／0／0 | 5／0／0 |
| OOBE 配置生成 | 15／0／0 | 无相关后续改动，未重复 |

配置测试中的子进程结果不重复计入顶层11条。最终 Server 无默认功能 all-targets 检查、比较诊断编译、格式与服务边界检查均通过，源输入在各次运行前后相同。没有重跑整仓、Linux或硬件测试。临时数据库核验其他连接和非默认schema均为0后删除，生产库未改。见 [v2记录](../../run/jev-routing-20261104/validation-v2/summary.json)、[最终影响验证](../../run/jev-routing-20261104/validation-v4/summary.json)、[编译与静态检查](../../run/jev-routing-20261104/comparison/build-v2/build-summary.json)及 [清理记录](../../run/jev-routing-20261104/validation/database-cleanup.json)。

以下首次失败原样保留：

- [首次路由单元 14／1／0](../../run/jev-routing-20261104/validation/routing-unit.log)：旧断言要求简单偏好升级，和新业务要求相冲突；改为同时断言近期上下文／偏好可由 minor 回答、复杂分析才升级、缺对象不猜测。
- [无默认功能首次编译 exit101](../../run/jev-routing-20261104/comparison/no-default-check.log)：配置校验误用了可选 `reqwest`，改为必需的 Rig HTTP 库导出类型，没有新增依赖或开更多feature掩盖。
- [validation-v3 11／8／0](../../run/jev-routing-20261104/validation-v3/routing-unit.log)：8项都在 loopback 端口绑定时被执行沙箱以 EPERM 拒绝，尚未执行协议断言；v4只恢复所需本地网络执行权限，源码、期限、并发不变。

另外在最终审查中修正了两项具体问题：环境密钥不得在配置加载时复制进可持久化设置；Jev不得保留已被热更新替换的聊天模型／预算。相应用例通过实际环境轮换和 `LlmService.apply_settings` 覆盖。未执行长期分类质量、忙时并发或真人整链；十条中文不能替代这些验收。
