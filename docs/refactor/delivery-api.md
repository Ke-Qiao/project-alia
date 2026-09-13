# Response 与 Delivery 操作契约

状态：2026-09-13，音频、文本与身体投递接口已实现并通过 macOS 整包及真实 HTTP 回归，测试证据见[构建记录](progress.md)。不代表实际声卡或机构已验收。

一个 Response 对应一次规范响应，拥有最初接纳的用户、请求、来源版本和生成意图。逐句 TTS 的每个 Delivery 保存同一 Response 下不可变的文本片段。Response 最终正文与助手会话消息在同一事务提交；重试返回原消息 UUID，不能产生第二份历史。取消或设备失联不会改写已经发生的工具 Effect。

## 身份与查询

所有接口要求当前活跃账户的 Bearer token，仅返回该账户的记录。管理员身份不授予读取其他用户响应正文的权利。不存在和其他 owner 的 UUID 都返回 404；无效 UUID 返回 400。接口没有重新发送或自动重播音频的操作。

| 方法和路径 | 结果 |
| --- | --- |
| `GET /api/v1/responses/{id}` | Response 生成意图、来源和最终内容/历史状态；不代表设备已播放。 |
| `GET /api/v1/responses/{id}/deliveries` | 同一响应的 Delivery 集合；每个响应最多 256 项，准备阶段执行此上限。 |
| `GET /api/v1/responses/{id}/events?after=0&limit=64` | 依 event_id 升序读取该 owner 的审计，包含 Unknown 后取得的 late_checkpoint；用末尾 event_id 继续，limit 为 1–256。 |
| `GET /api/v1/deliveries/{id}` | 独立投递身份、已知采样帧前缀、回执序号和终止原因。 |
| `GET /api/v1/deliveries/{id}/text-chunks?limit=64` | 文本投递的不可变 UTF-8 块、sequence 和字节 offset。省略 after 从首块开始；之后传最后的 sequence，例如 after=0；limit 为 1–256。 |
| `GET /api/v1/responses/{id}/body-deliveries` | 同一 Response 的身体命令、设备原始身份和事实，最多 32 项。 |
| `GET /api/v1/body-deliveries/{id}` | 身体独立账本；Submitted 只到已标明的硬件提交边界，位置确认仍为 null。 |
| `POST /api/v1/responses/{id}/cancel` | 请求取消该响应的未完成投递；结果中的 cancel_requested 不是设备取消 ACK，随后查询各 Delivery。 |

数据库不可用或提交 ACK 超时返回 503 与 delivery_unavailable 错误码，隐藏连接诊断。请求可能已提交，调用方应查询原响应/投递。不能用重新播放来判断上一次播放是否发生。

文本 Submitted 需要明确 End 和全部源字节的 native 写入证据，不证明读取；没有独立读取证据时不返回 Completed。身体事实以动作、帧和部分通道计数，绝不复用音频采样数。Response cancel 同时撤销两类输出的新准入，但返回值仍不是设备 quiescence/物理停止证明。身体本地 Unknown 后的回执在 Response events 中以 body_late_checkpoint 保存，保持原终态不变；查询返回独立 latest_checkpoint，重启时可据此恢复已确认的软件停止事实，不重写原 progress 或取得发送许可。

## 四种分别保存的事实

Response 的 pending 表示生成意图已持久化，但完整助手历史尚未提交。committed、interrupted、discarded 表示规范历史的最终处置。已提交的 Response 可以同时拥有 submitted、cancelled 或 unknown 的 Delivery。

| Delivery 状态 | 所需证据 |
| --- | --- |
| pending | 不可变目标与内容已保存，尚无外部发送许可。 |
| sending | 数据库 CAS 已授予唯一监督者发送许可；不能因为暂未见回执而重发。 |
| observing | 输入已被接收，仍在等待后端提交或呈现证据。 |
| submitted | 输入结束且全部源帧已提交指定音频后端；呈现仍可未知。 |
| completed | 输入结束且可验证的设备呈现证据覆盖全部源帧。本轮 CPAL/ALSA 不具备该证据。 |
| cancelled | 未发送的投递已撤销，或精确播放控制已确认；保留已知提交/呈现前缀，不能承诺硬件缓冲尾音为零。 |
| failed | 有明确故障事实；已发生的前缀仍保留。 |
| unknown | 断线、超时或缺少协议证据，最终结果不能确认；禁止自动重播。 |

submitted 是观察结束的明确结果，不能驱动 PostSpeech。只有完整、有效的 presented 证据能触发依赖“播完”的行为。ConversationTurnCompleted 仍只表示会话处理/历史提交，不表示用户听完。

## 播放协议与所有权

Playback contract version 2 要求 Start 接收确认之后才发送 Audio。回执携带原 VoiceScope、playback_id、playback_sequence、严格递增 receipt_sequence，以及后端实例和输出格式。End 仅关闭输入；回执流继续传递 submitted 进度，直到明确观察终态。心跳不能无限延长总观察期限。

所有计数使用均值下混后的源 PCM 帧。原始立体声一帧仍算一帧；设备输出声道展开与重采样不改变此单位。计数只可单调增加，并满足 presented ≤ submitted ≤ accepted ≤ offered。缺少证据为 null；旧传输连完整 progress 都缺失，不能把空字段解释为已确认的零或成功。

CPAL callback 缓冲写入和 ALSA writei 成功只能证明 submitted。回调静音补零不计入源音频前缀。设备格式与实例不能在同一播放中悄然更换；重采样按同一个流保持相位和源位置映射。真正设备呈现时钟与 AEC 仍须后续硬件证据。

Server 在网络发送前分配播放 UUID、预留本地序号并保存 Delivery；格式和 offered 上界写入完成后才发送相应媒体。调用方丢弃等待 future 不会丢弃监督者。关闭时先拒绝新投递，再等待监督、控制和回执任务，最后关闭渠道和数据库。等待预算耗尽只报告 incomplete，不能释放仍由原生工作占用的依赖。

单条取消包含精确 playback_id/sequence；需等待已进入的后端写入收尾，并返回 final_progress，不能用“停止当前音频”或空 ACK 冒充取消证据。Push 与精准取消共享设备原始单调序号，ACK 丢失后的快照不重新编号。整个会话退休继续使用原 scope fence；迟到取消不能影响同 epoch 的另一条播放。已经 Unknown 的状态不会因迟到证据复活，真实迟到快照保存为独立审计，可通过 events 接口读取。恢复仅处理已过期 owner，永不重放音频。

旧来源 epoch 不能被投递延迟重新盖章。新响应、媒体准入和最终历史提交核对原版本；已发生的进度事实即使随后撤权或遗忘也必须保存。缓存、ContextPack 和 Companion 的完整来源传播仍是下一主线，不能由这一检查替代。
