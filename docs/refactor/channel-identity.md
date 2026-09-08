# 设备通道与用户身份

Server 在 `[[channels]]` 中使用 `owner_user_id` 指定设备或 CLI 对应的账户。设备密钥证明消息来自已绑定设备；它不证明当前说话者身份，也不自动授予管理员权限。本轮不添加声纹识别作为授权条件。

配置流程不依赖前端：

1. 启动 Server，使用 `POST /api/v1/auth/register` 创建账户，或使用 `POST /api/v1/auth/login` 登录现有账户。
2. 从返回值 `user.id` 或已认证的 `GET /api/v1/users/me` 获取账户 UUID。
3. 将该 UUID 写入对应通道的 `owner_user_id`，重启 Server 以载入新配置。配置变更不是热加载操作。
4. `GET /api/v1/channels` 返回通道的 `owner_user_id`、运行状态和健康状态。通道重启接口要求管理员权限，且重新验证配置账户仍有效。

```toml
[[channels]]
type = "embedded"
id = "pi-living-room"
enabled = true
# 填写步骤 2 得到的实际 UUID，不要复制示例中的虚构账户。
owner_user_id = "<existing-active-user-uuid>"
endpoint = ""
```

CLI 使用相同字段。未绑定的 CLI 可以显示帮助、清屏和退出，但不能发起对话或创建/切换对话会话。未绑定的设备可执行既有配对和传感器流程，但语音输入返回 `FAILED_PRECONDITION / identity_unlinked`。Server 不再为语音自动制造 `embedded-user` 或为终端自动制造 `cli-user` 账户。

Server 构建时核对已配置账户是否存在且状态为 `active`。真实 Embedded 与 CLI 在接收对话输入时再次查询账户状态；持续语音的定期复核、创建响应及旧上传结束时也会复查。账户缺失、被停用或目录查询失败都不继续接收输入。工具批准及分派还会重新读取账户状态、角色和工具权限；不能仅依赖启动时的检查。审批仍绑定具体用户、工具实现、参数与意图摘要。高风险工具的确认必须经过已认证的执行接口，设备音频内容不等同于 REST 确认。

旧 `CapturedAudioChunk.user_id` 和 `InterruptActiveTurnRequest.user_id` 仅是被忽略的兼容字段，不参与选择账户。旧音频上传在结束、发布事件之前再次验证设备绑定，并核对 owner 没有改变。连续语音会话在 Ready 之前解析 owner，在凭证复核和每次创建新响应之前检查绑定；原始账户随已接收的输入传给 Interaction。

验证入口：`cargo test -p alia-server --test ingress_identity --locked` 使用真实 loopback gRPC 与签名凭证，检查未绑定拒绝、上传和打断不能冒用用户，以及上传期间撤销绑定后不发布音频。该测试不需要外设；实际设备配对和用户声纹不在此测试结论内。
