# Plugin contract — Embedded packages and lifecycle

2026-10-17。本轮仅定稿契约，不实现运行器、启动子进程、运行Cargo／服务／硬件或新增依赖。用户已明确未来允许安装未经审查的第三方代码；这要求区分包接收与代码执行，不能将当前同用户子进程运行器称为沙箱。R03保持暂停。

## Checklist

- [x] 复核manifest、包安装、实例启动、命令写入、输出事件、节点清理与Server调用点。
- [x] 将既有实现、源码静态缺口、尚未动态复现和目标契约分别记录。
- [x] 提出包／版本／实例／命令身份与失败清理目标。
- [x] 明确旧协议兼容边界及两端调用影响。
- [x] 给出无硬件的下一轮确定性测试清单。
- [x] 按用户已确认的D01管理主体、D02运行模式、D03升级策略对齐最终目标。
- [x] 独立技术核对`docs/architecture/plugin-contract.md`，对齐Embedded边界并冻结本文交Root收口。

本分工契约交接进度7/7；D01–D03已获用户确认并回填，不代表实现或动态验收完成。以下目标以总契约为准；实际隔离后端仍未选定。

## 当前实际能力

`crates/proto/proto/plugin.proto:5`已有Install、Uninstall、List、Start、Stop、SendCommand、StreamEvents七类RPC；`packages/alia-embedded/src/grpc/plugin_service.rs:63`起各入口调用绑定控制token验证。该认证证明调用者拥有当前设备控制凭据，不能单凭它证明上传代码可信或已隔离。

`plugin/manifest.rs:7`解析`manifest.toml`中的name／version／type／entry、资源和能力。`plugin/registry.rs:65`扫描只将manifest加入Installed，不自动执行；`registry.rs:677`按扩展名选择python3、sh或可执行文件，stdin逐行JSON，stdout／stderr转事件。未实现WASM／Rust动态库加载、独立依赖环境或权限沙箱。

`module/builder.rs:407`在扫描前将PluginCleanup注册到节点关闭owner；`module/registry.rs:124`请求停止并等注册表观察状态离开Starting／Running／Stopping。已有这一清理层，不能说完全没有退出等待。但实际wait／stdout／stderr任务由registry内部裸spawn，插件的原Child与task join没有暴露为精确实例的可重等完成句柄。

本轮复核没有发现Embedded插件专属测试；已有config/runtime覆盖插件目录配置，2026-10-17release生命周期原件使用空插件目录，只认证无插件输入路径。以下静态缺口尚未通过插件动态RED复现，不应冒称已经实测泄漏或攻击成功。

## 已确认代码形态与待复现缺口

| 范围 | 已确认源码形态 | 下一轮要实证的风险 |
|---|---|---|
| 包目录与解压 | `registry.rs:118`直接join请求name，原地创建／解压；仅ZIP entry的enclosed_name及词法路径检查；`178`只验证manifest.entry存在 | name／entry／现存链接逃逸、坏包留下半安装、同名包覆盖；未见累计解压字节数／文件数等限制。不要将transport默认消息上限等同解压预算。 |
| 包身份 | proto Install带version，但service第69行只传name和zip；registry按请求name登记，manifest可能另报name；没有内容摘要身份 | 请求／manifest名称版本不一致、同版本换字节、重启scan身份变化。 |
| 启停并发 | `registry.rs:235`只拒Running／Starting，允许Stopping重新start；spawn失败已先将状态设Starting；`545`原wait只按name回写 | spawn失败残留Starting、旧进程退出覆盖新实例、等待期间remove／install替换注册项。 |
| stop／uninstall | `291`发SIGTERM不等退出且未检查kill返回；`215`卸载只拒Running，之后先删map再删目录 | Stopping时目录被删、删除失败后登记丢失、进程未退出却返回停止请求成功、原owner结果丢失。 |
| 命令 | `322`读当前stdin后释放map锁，再等待stdin mutex并写JSON；无命令回执／取消／稳定摘要，`plugin_service.rs:184`返回sent | 等待时实例更换／权限撤回；写入不等于插件执行；半写／连接中断后的执行结果未知。 |
| 资源与输出 | manifest的CPU／内存／gpio／usb／network是声明，`registry.rs:246`没有执行隔离／环境清理；`497`lines读取未见单行预算 | 同用户文件／环境访问、无换行大输出、堵塞stdin、孙进程持有pipe；process边界不能冒称权限隔离。 |
| 事件流 | `plugin_service.rs:197`首次鉴权后裸spawn，recv lag直接结束，无显式cursor／gap；registry事件只含name／type／timestamp | 无事件时客户端断开资源不及时退出、旧实例事件混入、漏事件被误解为连续完整记录。 |

精确Server影响：`packages/alia-server/src/channels/embedded/channel.rs:1289`将安装name／version／ZIP转发；`1332`及`1348`按name启停；`1364`的send_plugin_command在每次调用第1374行新建UUID。后续幂等重试不能沿用“每次转发另造ID”，应由逻辑请求owner保留ID并透传；本轮未修改该行为。

## 目标身份与兼容契约

以下为目标，不是现有实现。

1. **扩展与包身份**：宿主控制的稳定namespace／安装身份防止同名冒充或接管；严格合法的package name、显式version、实际ZIP字节SHA-256及字节数描述不可变安装内容。name／version是人类标签，digest只证明内容身份，不授予信任；不得让模型或包自报摘要建立执行许可。安装请求声明必须与实际manifest匹配，另记录验证后的文件清单／入口关联；禁止在未定义规范化算法时声称不同ZIP字节“内容等价”。
2. **安装与选择**：已验证包进入不可变版本目录；运行选择指向确切已安装身份。安装成功不自动选择或启动；未审查包保留未审查状态，不因安装者是管理员或包签名存在变成受信代码。安装目录、每次运行临时目录及可持久写数据分别归属明确owner，禁止直接覆盖当前运行内容。
3. **部署、授权与运行实例**：部署身份包含包、实际入口、ABI、锁定解释器／依赖环境修订或隔离镜像内容；仅固定脚本而仍从可变环境加载依赖不足以固定实现。授权另绑定主体、资产、运行模式、能力／资源scope及policy/grant修订。每次Start生成独立run ID，绑定本次device／host boot及部署身份。PID仅是进程句柄附带信息，不作为身份。状态更新、日志、回执、停止及清理必须匹配原run ID，名称重用不能让旧wait覆盖新实例。
4. **命令与目录身份**：命令绑定run ID、稳定command／EffectRun ID、调用主体和scope、规范action／payload摘要及原始截止时间；目录另绑定工具定义、注册修订、实现digest及协议能力。同ID同内容重试返回既有状态；同ID异内容拒绝，不静默重发。新增事实接入现有Execution的build／registration／implementation身份和批准账本，不另建第二套效果账本。运行环境、grant或schema变化均须重新核对批准，不能仅凭包字节相同复用。重启后不将旧命令转投新run。不可承诺未经审查插件对外部系统的exactly-once；无终态证据时明确Unknown，后续查询／人工判断，不自动重放副作用。
5. **协议相容性**：manifest／运行协议需要显式版本和握手能力；未知必需版本／能力拒绝启动。旧name-only RPC不能静默映射到最新运行实例并获得新权限。允许保留只读列表或离线导入旧manifest供诊断，但执行需要明确迁移后的身份和隔离条件；没有“兼容失败就退回旧同用户runner”。具体proto包名／版本号与Root的统一契约对齐，本轮不提前生成代码。

CPU／内存／文件／网络／设备权限必须是宿主实际可实施的限制，而非插件声明。D02已确认默认隔离执行；隔离后端未选定、不可用、能力不足或未通过验收时，第三方包可以处于已接收／不可运行状态，Start应明确拒绝。另设必须单独明确授予宿主信任的受信本机模式，该模式不能冒称同用户恶意代码仍被应用内代理隔离。绝不自动降级为当前进程用户运行；本分工不预建多后端框架。

## 生命周期与失败清理目标

包安装使用任务专属暂存目录、固定预算和唯一owner：验证名称／版本／入口、普通文件边界、链接、文件数及展开总量后才发布为未激活；不执行entry、安装钩子或包脚本，不自动下载依赖。失败保留原错误并清理本次暂存，不能删除已存在版本。spawn_blocking解压future被调用方取消不代表原工作已停止，owner须保留实际完成句柄；结束前不能删除正在写的目录或启动同目标覆盖。发布后持久性未确认必须明确返回不确定阶段，核对相同操作与内容再恢复，不能盲目当成“未安装”重新覆盖。

建议运行状态为Starting→Ready/Running→Stopping→Exited/Failed，并独立报告CleanupPending／CleanupComplete；状态名最终随proto统一，但语义不可折叠：

- Start先登记实例owner，再创建进程／pipe／工作目录。spawn或握手失败必须走原实例rollback；握手成功前不接收执行命令。宿主确认ready只证明该实例协议握手，不能证明插件声称的物理效果。
- 新命令准入关闭与stop请求为同一实例的明确边界；已阻塞的写入在真正发送前检查实例／grant仍有效，取消不能转到另一个run。stdin写入和输出解析各有有界字节预算与期限，并归本次owner。
- Stop响应区分已请求与已完成；完成必须观察原child退出、必要pipe／reader／writer退出及后台owner收口。客户端取消等待仅取消waiter，不能detach原owner；以后用原operation/run ID可继续查询或等待。信号发送失败、非零退出与cleanup失败均保留，不能覆盖首个原错误。
- 原节点shutdown grace不提高；超过grace报告未完成，原完成句柄继续保留，不能因超时或强杀请求已发就宣称退出。是否使用后续受约束强制终止由隔离／进程树控制契约决定；不是本轮擅自增加杀进程动作。
- 在旧run尚未完全清理时，不允许同一个活动槽复用其身份或卸载其包。目录删除成功和注册记录持久完成一起构成卸载完成；文件删除失败保留可重试状态，避免先遗失注册信息。
- D03已确认升级先关闭新调用准入，保留旧run与原owner，在旧调用的原期限内排空并完成真实清理后才切换；不因升级延长期限，预算耗尽而清理未完成时继续报告未完成并保留原owner与包引用。停用／撤权则关闭准入、取消相关调用并等待原owner真实清理。不能原地覆盖运行文件、删除未清理旧包、把旧回执应用到新run，或在新启动失败后谎报升级成功；新版本不继承旧批准。
- 回退另为显式管理操作；不得恢复旧批准、绕过撤权或盲目降级数据库schema。新版本及新环境使用自己的身份，不继承旧运行许可。

## 命令与事件的可观察语义

命令至少区分Rejected、Accepted、Running、Succeeded、Failed、CancelRequested、Cancelled以及Unknown；具体编码可缩减，但必须能区分“宿主接收／写入”与“插件回执／宿主观测”。Accepted不算业务成功，CancelRequested不算副作用已撤销。插件自报Succeeded只是该插件的执行声明，涉及硬件或受保护外部动作时仍须既有broker／批准／receipt证据，不能让任意JSON绕过BodyControl或当前政策。

取消对象为确切run ID＋command／EffectRun ID；重复取消可幂等查询原结果，迟到完成与取消按真实事件记录，不能改写已发生结果。撤权禁止新的内容采纳和副作用，但已经dispatch的效果所产生的真实迟到回执、失败及资源清理事实，仍须按原命令／运行／EffectRun身份入账；保存事实不恢复授权，也不转成新run的成功或触发重放。插件退出而未给可信终态的在途命令收为Unknown或明确的执行中断，不自动新建命令重放。命令保留原deadline，不随重试、升级、重连续期。

stdout协议帧与stderr日志需要有界帧长、累计队列及明确溢出策略；事件附run ID和单调序号。慢订阅者应收到明确gap／断开原因，禁止用timestamp推断无丢失。订阅任务随客户端断开和宿主关闭退出；长期流的鉴权撤销语义与Root权限契约统一。重连不宣称持久回放能力，除非以后确实实现并验证有界日志存储。

## 下一轮最小实现和测试影响

本轮只设计，下面测试尚未写入／执行。先用临时目录和受控无设备fixture复现确认的缺口，再实现；不需要传感器、动作或网络外设。

| 顺序 | 必需测试组 | 证据要求 |
|---|---|---|
| E1 包接收／发布 | name／version与manifest不符；name／entry绝对路径和父目录；现存链接；重复文件；坏manifest；压缩包超文件数／总展开量；取消发生于受控解压阶段 | 暂存及原版本独立，原失败保留，原blocking owner完成后才清理；不得靠sleep猜阶段。 |
| E2 原实例所有权 | spawn失败、未握手退出、Starting期间stop、Stopping期间start／uninstall、旧wait／旧stdout迟到、新包同名安装 | 精确run ID不串代；原Child和全部pipe任务可重等join；无孤儿进程或目录。 |
| E3 命令语义 | 稳定ID重复／异内容冲突；stdin堵塞／半写；等待期间stop／grant撤销；成功、失败、迟到回执、取消与结果未知 | 不把sent当完成；旧命令不跨run；撤权后已dispatch的真实迟到事实仍归原身份、不恢复授权；不重复副作用；原deadline保留。 |
| E4 输出／流清理 | 超长无换行输出、坏JSON、事件lag、无新事件时客户端断开、旧实例事件、进程派生子任务持有pipe | 字节和队列有界，gap可见；完成前证明原owner/任务收口。 |
| E5 升级与第三方执行准入 | 升级关闭新准入，旧调用按原期限排空并真实清理后切换；超期保留未完成状态与原owner／包引用；停用／撤权取消并清理；隔离缺失或能力不满足时Start失败 | 新版本不继承旧批准；未清理旧包不能删除；未审查包绝不退回旧runner；受信本机模式须单独明确授予宿主信任；显式回退不复活权限；必要真实隔离验收待选型，不能用mock证明沙箱有效。 |
| E6 节点与两端API | PluginManager失败码／operation状态、Server稳定ID透传、节点shutdown原grace、取消waiter再重等、重启scan恢复包状态 | 复用既有节点cleanup，先排空再查资源；旧name-only客户端明确拒绝或只读，不静默执行。 |

预计修改点仅在获授权实施后收敛：proto插件契约、Embedded manifest／registry／plugin_service、节点PluginCleanup及Server嵌入通道转发；需要按具体隔离决定再确认额外文件。普通BodyControl、默认shadow、执行批准、R03暂停、真实硬件范围均不因插件契约自动改变。

## 独立审阅与已确认决定

已独立技术核对`docs/architecture/plugin-contract.md`的安装、身份、生命周期、回执与Embedded实施映射，未发现阻止本轮文档收口的技术矛盾。总契约与本文均为目标；既有源码差异仍是静态核对，未执行新的动态复现。用户已确认以下三项，本文完成回填后冻结：

- D01：管理员管理部署／授权／跨用户数据；普通用户只使用获授权能力及本人隔离数据入口，调用仍受当前政策和执行批准约束。
- D02：默认隔离运行、不可用即拒绝；另设必须单独明确授予宿主信任的受信本机模式，不自动降级。实际隔离后端仍待后续选型与验收。
- D03：升级关闭新准入，旧调用在原期限内排空并真实清理后切换；停用／撤权取消并清理。保留旧run原owner和未清理包引用，新版本不继承旧批准；已dispatch的真实迟到事实按原身份保留，不恢复授权。

本分工不自行选择容器／WASM／新语言依赖，不启动任何实验。强制终止的实际机制仍待隔离／进程树契约确定；其余目标也尚未实现或动态验收。本次仅更新本HANDOFF，未修改生产、proto、测试或全局契约文件。
