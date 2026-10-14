# R02 R1 参考路由与 consumer 前置 — 独立审查交接

日期：2026-10-14。分工：`settings_auth_review`。本轮只核对 R1 硬件 AEC 路径的 consumer 决策条件与证据边界，沿用已完成的 R02 clock，不重复实现时钟/参考队列。仅编辑本文；不改源码/测试、不运行 Cargo/Git/SSH/Docker/模型，不打开设备或录放音，不采购、接线、添加依赖或选定算法。R03 继续暂停。Root 负责中央文档、验证与提交。

## Checklist

- [x] 读取当前时钟设计、前轮 review HANDOFF 与现有硬件确认，区分历史验证与现场未证事项。
- [x] 对照当前代码和已固定依赖 API，核对真实 frame/rate/channel/delay 与消费入口。
- [x] 明确硬件 AEC 不自动叠加主机 AEC，列出未来软件 consumer 的最少决策项。
- [x] 向 Root 汇报文档不一致、配置/能力混淆风险及不阻塞当前资料核对的边界。
- [x] 独立读回本轮中央文档与其他分工证据，确认无夸大验收或隐含新硬件动作。
- [x] 汇总最终结论并停写，Root 统一 checklist、检查点和提交。

当前 **6/6**，仅为本分工交接进度，不是硬件 AEC 验收比例。独立审查无阻断项；本文停写。当前无需新增算法实现或测试，Root 统一完成最终文档检查、用户决定状态和提交。

## 当前可以确认的分工

R1 尚未接入 Pi，计划通过 USB；麦克风和扬声器均由 Pi 连接，扬声器/功放未定。以上沿用用户已确认事实。前轮硬件 audio HANDOFF 已从亚博对应 R1 资料区分 UAC 处理后采集输出和独立功放输出参考接口；本分工本轮未再次访问网页，不把旧官方页面说明冒充实物或当前固件测量。USB 连接、同一主机和产品支持 AEC 都不能证明参考已进入芯片或算法已启用。

硬件路径的条件性顺序是：确认实际板/固件与处理输出模式；确认厂商允许的参考来源、电气条件和最终扬声器链；后续在明确现场窗口证明参考路由与声学效果。如果硬件处理满足目标，现有主机可继续消费其实际处理输出并保留诊断，不需要为了架构完整性自动叠加一个软件 AEC。当前没有证据支持选择“双重 AEC”，也不能通过软件旁路配置把硬件效果标为主机算法效果。

本项目已存在 native capture、voice/cue mixer、最终 postmix render reference、独立时钟、单 reader、有界证据导出/重放和按身份精确取消。它们是软件前置能力，不能替代板端 AEC 配置、参考输入电气匹配、采播校准或双讲验收。前轮 clock 的默认335/0/0、无默认260/0/0属于历史已完成软件证据，本轮不重复运行、不算作新现场通过。

## 已核对的代码和固定依赖

| 项目 | 当前代码事实 | 本轮不能推导的内容 |
| --- | --- | --- |
| feature/模块 | Embedded features 为 sherpa-onnx、vision-opencv；没有 AEC feature/processor/自动 worker | 没有已选定主机 AEC；Cargo feature 不是硬件启用状态或声学证明 |
| CPAL 0.15.3 / ALSA 0.9 | 实际流选择、输入/输出与后端时间观察；CPAL 允许格式 fallback，ALSA 使用 near 参数选择 | 请求格式不是设备支持清单或实际运行格式；ALSA `hw`/插件路径也不能只凭配置名推断 |
| rubato 精确5.0.0 | playback 使用 Async sinc，固定比率、最大相对比率1.0、约10ms整数内部块，修剪 output_delay | 不是 AEC，也没有动态 ratio 反馈控制；内部块不是 consumer 帧 |
| capture converter | native tap 后先把声道求平均为mono，再固定比例线性插值和目标声道复制/增益 | 未校准第一native样本到第一converted样本；多声道含义未知时不能把不同信号盲目平均 |
| Sherpa | 现有 KWS/VAD；VAD 自身按 window_size 缓冲，默认512 | 该窗口不是未来 AEC 的帧要求，未运行模型确认本轮效果 |
| 当前 reference consumer | 显式 EvidenceSession 排他取得 reader；无自动 AEC 消费者 | 取得 reader 不会把现有 converted speech 路径自动改为 AEC 输出 |

已只读核对本机 Cargo registry 中 rubato 5.0.0 的 `Resampler::output_delay` 和 `Adjustable` API：库允许支持者由外部反馈调节比率，但项目 wrapper 没有调用该能力。库报告的转换延迟与实际 ADC/DAC、USB、功放、R1 处理及声学传播延迟不是同一个量；修剪转换器 delay 不构成整条链路延迟校准。没有下载、升级依赖，也未引入 WebRTC/Speex 比较或帧长结论。

## 格式与声道的准确前置条件

`SpeechConfig::validate` 和 `CaptureClock` 明确要求**转换后 16 kHz mono**；每个上传块最多1600样本，短块不补齐。默认 `AudioConfig` 请求 capture16k/1、playback24k/1，但 native 格式由实际后端协商。Core 的8–384kHz、1–32channels是证据格式允许范围，不能写成 R1、声卡或某个未选算法支持这些组合。

native handoff 的1024单位是**交错样本**，不是帧或毫秒；capture/playback共享64块。各 callback/read 的块长不固定。`opus_frame_ms` 虽保留该字段名，当前唯一生产读取是 ALSA capture period 请求；CPAL capture 使用选定配置，不读取该字段。speech 上传明确是 PCM，不能据该字段把20ms当作上传固定帧，更不能作为 AEC 帧选型依据。

“六麦”描述阵列物理输入，不能推断 USB 暴露六路未处理 PCM。当前旧官方依据描述处理后的单通道 UAC，但仍需实物端点/固件确认。若今后端点提供 raw、processed、reference 或其他通道组合，必须先确定每路信号含义、位深/对齐、增益和处理状态；现有 converter 的平均策略不能自动作为这种多信号端点的合理选路方案。本轮只记录门槛，不改通道适配。

R1参考若来自功放输出，软件 postmix tap只记录主机实际提交内容；它不能覆盖或证明之后的硬件音量、功放限幅/处理、电气取样和声学传播。最终参考应涵盖实际播出的 voice/cue 等内容，但必须以厂商适用资料和现场链路确认。当前不假设 USB playback endpoint、不把 USB自动视为内部reference，不指定接线针脚、电平、接地或功放型号。

独立电气参考与主机诊断队列必须分开：AudioReferenceBus 的 queue loss/ClockDiscontinuity 说明主机观察或未来软件consumer不能沿用先前数据/映射，不证明 R1 芯片实际丢了电气参考；host 队列连续也不证明 R1 参考已接通。当前没有据此控制或重置硬件 AEC 的协议，不能把软件 replay reset 变成板端动作。硬件重新收敛和软件观察状态是不同验收对象。

## 若未来决定采用软件 consumer，最少需要的决策

| 决策项 | 必须明确的输入 | 为什么现有代码不能代替 |
| --- | --- | --- |
| 是否需要主机算法 | R1 AEC启用/旁路/门控、输出是何种处理结果、真实目标缺口及是否允许叠加 | 设备能力声明和软件mixer不能证明需要二级处理 |
| capture输入与输出 | 原生/转换后插入点；每路含义；支持rate/channel/sample表示；输出是否送往VAD/KWS/upload | reference与converted AudioBus为独立路径，不能仅加reader即完成接入 |
| render参考来源 | 实际最终mix/volume/格式量化边界；是否有未覆盖的系统或硬件音源 | 原始TTS与配置音量不是实际最终参考；硬件后级变化尚未包含 |
| 算法帧与调用契约 | 支持的格式、帧大小、render/capture先后、最大等待、partial/tail、缺参考时行为 | 1024样本/上传1600/VAD512/rubato内部块都不是算法规范 |
| 时间与延迟 | 固定stream/continuity片段、原始锚点和精度、转换历史、可失效offset/slope、实际路由/延迟证据 | host观察时间和相同名义rate不能证明跨流对齐；名义分块offset不是独立测量 |
| 资源与故障 | 唯一reader owner、有界缓存/worker、gain/format变化、loss/xrun/reopen/clock reset、取消退出与重收敛 | 不能争抢第二reader、补未知静音、复用失效映射或把观察取消当播放取消 |
| 验收标准 | 选定硬件/算法后明确回声抑制、双讲保真、唤醒/语音路径、端到端延迟和恢复要求 | 合成测试只能验证软件事实，不能提供声学阈值或硬件结论 |

这些是未来工作的准入资料，不是本轮实现任务或新的公开接口。当前不创建占位consumer、额外registry、feature、线程或配置。软件 consumer 若被选定，才决定必要转换、组帧和下游唯一有效信号；诊断应服从其唯一owner，而不是另外消费同一native队列。精确播放取消和 native owner 关闭仍各自保留原边界。

## 首次独立反馈与非阻塞边界

已向 Root 报告一处文档不一致：`audio-reference-clock-design.md` 旧“实现范围与后续主线”段仍无条件要求接入主机 AEC worker，后面的新契约却正确保留“是否需要软件 AEC 未决”。建议前段改为条件式：先核对R1模式/参考，再决定软件consumer；仅在作出该决定后讨论软件worker/组帧/映射。Root负责修改中央文档，本文不代改。

缺失的板/固件、USB端点、原生格式/声道、参考电气、扬声器/功放、AEC启用状态和实际延迟，阻塞实物兼容性、算法/路线决策与声学验收，但**不阻塞本轮文档核对和条件清单**。用户已经明确R1尚未连接、R03暂停，无需为了完成本轮重复询问这些事实。后续需要新增资料时应集中索取最小缺项，不用配置样例、同品牌商品、相似阵列或软件回归填成已验证。

## 本轮证据口径

本分工依据当前仓库源码、固定依赖本地源码、前轮HANDOFF及已保存硬件事实；没有本轮录放音/设备枚举/模型/测试结果。官方型号说明、用户确认、主机只读枚举、软件测试和声学实测须分别注明来源。最终中央文档应保留“未认证来源声明”“后端接纳不等于物理呈现”“支持AEC不等于已启用/有参考”“只读预检不等于硬件就绪”四条边界，不把资料齐全当成R02验收通过。

随后已只读核对 Root 本轮保存的 `run/validation-20261014-r02-r1/retrieval.json`、`image-retrieval.json`、介绍正文和三张原图。记录显示2026-10-14本轮公开 course/introduction/product与图片均HTTP200，原件留存；本分工没有另发网络请求。介绍正文明确单通道降噪UAC与功放音频参考分离，并把更多规格指向“附件→厂商资料”；没有给出实际采样率、位深、AEC启用模式或参考电气参数。UAC图仅给USB连接信号，reference图仅给左右功放输出正负信号；机械图给尺寸且示意板标H-6，不能据该标签认定用户实物版本。上述新增公开证据仍不支持默认USB playback、内部参考、接线电气或板端模式结论。

已完整读回中央 `r02-r1-aec-preflight.md` 初稿与主机分工 `host-HANDOFF.md`。候选拓扑、实际尚未核验、硬件/host参考分离、用户路线选择仍pending、延迟各段与帧粒度的表述未见阻断。已反馈两处精度：把 `opus_frame_ms` 明确收窄至 ALSA capture period；软件AEC后续条件应包括已确认的兼容性/模式不满足，不必等不适用硬件先产生声学结果才允许评估。Root 负责中央修订与旧时钟文档的条件式worker说明；等待最后一致性结果后收口。

## 最终独立结论

已进一步读取硬件分工 `hardware-HANDOFF.md` 与主机分工终稿。它们与中央文档对已证事实、条件性风险和未知项的区分一致，未把公开资料“未说明”写成功能必然不存在，未把当前多声道平均逻辑写成已经发生的 R1 硬件故障。中央文档已收窄 `opus_frame_ms` 到 ALSA capture period；时钟文档旧主线段已条件化为先核对硬件、仅确认需要软件AEC时才接入worker。硬件参考有效性与host队列丢块也分别记录，未追加板端reset动作。

独立用纯文件读取核对本轮六份官方原件：course/introduction/product JSON 与 UAC/reference/product-specs 三图的实际字节数和 SHA-256 **6/6 全部匹配** Root 获取记录，记录均为HTTP200。未重新请求网页、执行下载内容或重复任何设备操作。原件真实性范围仅是对应官方获取内容与存档一致；不能由hash推导实物/固件或声学状态。

本轮没有新编译、测试、设备枚举、声学或模型证据；只读审查未发现需要立即修改生产的已复现缺口。软件consumer仍未选定，不引入依赖、frame适配框架或处理线程。硬件优先路线与待补资料由Root集中处理；本文收口时用户尚未回复，维持pending，不把等待当成同意。Git提交与最终工作区核对由Root完成，本文不提前宣称已经提交。

最后已读回中央契约、当前恢复检查点、next-round、延期台账、硬件指南、包音频说明与 `evidence/validation.json`：中央已把后续软件路线条件扩为实际兼容性、模式或实测效果不满足；next-round不再强制先建主机consumer，R03暂停与R01—R06的2/6保持。Root静态记录核对基线 `9890bab823a2ab8eb49c3444ee29a65ebab86faa` 的660个非Markdown输入未变、210个相对链接无缺失、diff check exit0；本分工未运行Git复查，而独立对照前轮666文件冻结hash，唯一差异确为 `packages/alia-embedded/docs/audio-and-voice.md`，非Markdown差异为空。此项与六份来源原件独立hash核对共同支持本轮仅资料/文档变更；不把这些计为运行或硬件测试。

后续优先入口是实物/固件与厂商附件、处理输出/端点语义、功放参考电气及模式核对；这些到位后才决定是否需要主机算法或有依据的通道选择，再进入单独确定的现场验证。硬件AEC不自动叠加软件AEC，R03继续暂停。已向Root报告无阻断项，并停止编辑本文。
