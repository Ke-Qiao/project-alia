# D3 Server 身体命令接入与验收

本包实现服务端 Body client、持久 Delivery 接纳接口、channel 自有观察任务和 expression 适配。实际串口、PCA9685、舵机到位反馈和实物停止能力由 Embedded 驱动及硬件验收单独证明。以下软件事实不代表舵机位置完成。

## 原命令与观察 owner

`BodyDeliveryOrigin` 指向既有 Response 及 `full`、`pre_speech`、`speech_start`、`post_speech` 四种输出阶段。内部 `source_command_id` 固定为 `response_id:plan_id:phase`；非语音控制作用域使用原 Response UUID，语音沿用原 VoiceScope。完整动作内容、owner、plan、scope 与设备身份保存在 `BodyCommand`。

channel 先 lookup 原 Response/阶段，再为确实的新命令分配 control lease 和单调 sequence。已存在记录只能恢复原事实，不能先 Open 新 lease 或重新 Submit。新的身份分配、持久 claim 和首次 Submit 位于同一个 dispatch gate，保证各用户及 voice/nonvoice producer 共用有序序号。

内存最多保存 256 个 source 条目，同时运行的观察 owner 和等候队列分别限制为 32。只有写入者已有精确 quiescent 证明、持久账本已确认并且 owner 已退出的条目可以淘汰；再次出现时必须查原账本，不把缓存缺失当作新物理命令。未确认的身份不靠淘汰释放占用。

Unknown 的原 status/progress 保持冻结，独立 latest_checkpoint 返回已持久化的最新迟到设备事实。重启恢复和缓存缺失后的只读观察均使用该字段核验 quiescence，同时保留原 unknown_reason；已停止条目可以释放本地缓存，原动作不会因恢复获得重发许可。

首次等待及单次 gRPC deadline 为 1.5 秒。五秒动作由 channel 的独立 owner 持续观察，调用者 drop 不丢失该 owner。动作 native budget 为 30 秒，观察预算为 60 秒；超过预算而缺乏软件写入者退出证明时保留 owner 和 channel 依赖。shutdown 封接纳、取消原命令、真正 join 后才释放连接。关闭等待本身被取消，不会取走保存的 join handle。

首次回执可以已经包含 Started/Submitted；所有事实必须通过原 command、有效 sequence、动作计数、驱动边界和单调性校验。同一原始回执重复送达是幂等观察。开环驱动只报告 Submitted，`position_confirmed_actions` 始终为空，expression 的 `completion_acknowledged` 始终为 false。精确 quiescent 回执可以解除软件占用，不将提交解释成舵机到位。

## ACK 丢失、取消与撤权

Submit 只发送一次。缺少有效签名 ACK 时记录 Unknown，查询原命令并精确取消；不能将同一动作换成新 identity 重试。

Cancel 携带完整原始 `BodyCommand` 和 reason。设备可以在迟到的 Submit 完成准备前，为相同身份建立明确的拒绝事实。已置位取消意图但尚无 quiescent 终态时，服务端在有界间隔后幂等重试原 Cancel；第一次取消请求丢失不能变成只续租观察。观察失败或数据库 ACK 未知独立记录，不篡改设备 receipt sequence 或伪造 Cancelled。

持久 store 在物理提交前 claim/authorize，并在观察期间复核原 Response 的 owner、隐私 epoch 和取消状态。撤权或校验失败转入原命令取消。已经发生的合法 device progress 仍作为事实写入，即使原请求后来不再具有新输出权限。每次数据库操作采用 repository 的两秒确认上限。Server 初始化回收已过期记录，不把重启后的 Sending 行当成新的发送许可。

## 设备公钥固定

身体控制需要在对应 `[[channels]]` 中同时配置：

```toml
# 从设备本地可信输出核对后填写；不能直接信任网络发现结果。
expected_device_id = "<verified-device-id>"
expected_device_public_key = "<base64-encoded-32-byte-Ed25519-public-key>"
```

未配置、只配置一项、错误 base64/Ed25519 长度，或者设备状态与配置 pin 不一致，都不能取得身体控制能力。直接 `execute_expression` 同样检查真实 pinned transport，不能绕过能力协商。无 pin 不阻断当前音频通道的独立初始化。自动发现和未签名 GetBindingStatus 不得填写或替换 body pin；跨重连与重启始终使用持久配置中的设备公钥。

`alia.body.v1.BodyControl/Exchange` 对严格 JSON 的原始字节进行方向隔离签名，复用现有 Ed25519 工具。每次请求使用新的 UUID nonce、目标 device ID 与毫秒时间戳，设备校验 30 秒时效与真实 Bound bearer 身份。服务端验证设备 pin 签名、nonce、目标、版本及原命令事实。请求签名不能作为回执签名使用。

## 本包验证

最终测试结果以同包 `progress.md` 和根整合回归为准。客户端定向测试覆盖：五秒动作与等待者 drop、错误 nonce/设备/版本/签名方向/公钥、错误命令身份和伪造位置反馈、丢 Submit ACK、丢首次 Cancel、缓存淘汰后只读恢复、不同用户及 voice/nonvoice 的全设备序号、idle lease 到期、原 Response 撤权及 shutdown wait drop。另覆盖配置 pin 成对与格式校验、unsigned status 不得替换 pin，以及 expression 将 Accepted 观察为 Submitted 后释放软件占用而不宣称位置完成。

实际 Pi 的设备 ID/公钥、绑定状态、控制板和串口协议仍需实机提供。没有这些信息时，软件可完成构建与签名 loopback 验证，生产 body 能力仍保持不可用。
