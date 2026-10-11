# Settings GET 等待后身份重验证

本轮承接 `b15fa7d`，范围为 `GET /api/v1/settings` 等待后、返回敏感设置前的管理员身份核验。先审计与复现，再补实际缺口；保留 API、政策、现有管理员读取权限和依赖/平台范围。

GET 入口从数据库读取 AuthUser，并拒绝非管理员。随后依次等待更新 gate、SQL current、Registry 政策投影写锁以及 `list(prefix)` 或 `all()` 读锁。SettingEntry 的 `value` 直接进入 JSON；`schema.secret` 是 UI 元数据，不是服务端遮罩。原入口身份快照不能证明这些等待结束后仍是有效管理员。

最后准入点应位于设置快照收集完成后、同步 JSON 和 ETag 构造前，之后没有 handler 内 await。已复用上轮 PATCH 的 2 秒 active admin 查询为同文件私有函数；保持 PATCH 原检查位置、错误消息与政策事务核验。降权、停用、删除为 403，查询错误/超时为固定脱敏 503，错误响应不包含设置或政策 ETag。成功仍提供原值/schema、prefix 和政策版本 ETag。

拒绝前可能已经执行原 SQL 暂停到期转换或 Registry 政策投影；这些是原读取语义，不是本次重新授权所引入的写入。此轮保证的是最终授权前不交付敏感响应，不把身份核验变成持续撤权或跨网络事务。

实际复现、回归和同源双平台结果在 [checklist](checklists/settings-read-authorization-2026-10-12.md) 与 [四份 HANDOFF](handoffs/2026-10-12-settings-read-authorization/README.md) 收口。纯 gate 排队、Registry 私有锁若没有可靠动态见证，会明确记录静态覆盖边界，不加入生产测试钩子。

## 原实现复现

冻结 RED 655 输入，manifest `e9dbd269854da9e4562a48d763e1875380862dd83c52130c539983a435842161`；生产源相对基线零变更。实际政策 SQL 锁等待、更新 gate 占用且请求未结束后，另一连接提交管理员降权，再释放锁。原实现返回 200、带 ETag、包含随机假密钥，文件未变；预期 403 的断言失败。退出 101，19.53 秒，无源码漂移。原测试全文、命令、日志与生产基线核对保留；该结果是有效业务负例，不是测试屏障失败。

## 首次修复后验证

V1 定向 **5 通过/1 失败**；拒绝场景均通过，真实 users 锁等待 2006 ms 后返回无设置/ETag 的 503。唯一失败为正常管理员完整 JSON 与 Registry 内存值比较，当时尚未定位差异，未预判为夹具或生产问题。manifest `aa52f64b79e6ec2fd82bb79d1e1828066e3c0c954c4138d92ebb5c95d13aec36`，655 输入、41.22 秒、exit101，无漂移；Linux V1 仅准备快照和环境，没有执行 Cargo。

V2 仅添加不含设置值的诊断并保留原断言，单例再次失败（11.24 秒，无漂移）。166 个键完全对应，仅 3 项浮点 value 不等，schema 均相等；期望数据经过与 HTTP 同样的 JSON 编码/解码后，完整响应严格相等。因此 V3 仅将期望值按线上的 JSON 表示进行比较，保持全图严格相等，不忽略字段、不设浮点容差、不调整生产或依赖。原诊断日志保留。

## 最终验收

源码 `d080710052ae308b962b826d0f096fb10067469e`；最终 V3 manifest `48547827ad52952af80c9275cc83dd35ef9da5a25cb58b7fb32e8cab57fc1c7c`，655/655 源码 Git blobs 一致。

macOS ARM64 workspace **1134/0/5**；Linux ARM64 Server 默认完整回归 **852/0/5**。两端 Server 无默认 feature 全目标检查、架构边界正反自测、变更 Rust 格式、差异及计数器 8 例自测通过。数字为 passed/failed/ignored，嵌套子进程摘要不重复计入。全部最终阶段同源且无漂移，原期限与默认测试并发保留。新增 GET 6 项及既有 PATCH 8 项均在两端全量中逐项通过；定向 V3 的 6/0/0 不额外计入总数。

RED 在未改生产时有效复现降权后的 200/ETag/假密钥泄露。GREEN V1 为 5/1，V2 保留原断言的诊断单例为 0/1；166 个键中 3 个浮点 value 与直接内存值比较不同，但期望按 HTTP 同样 JSON 编码解码后完整严格相等。V3 只修正测试比较层次，不用容差或删除字段，生产自 V1 起未变。Linux V1 仅准备环境，未执行 Cargo；Linux 最终阶段明确使用 V3。失败、诊断、原 RED 测试全文和各 manifest 均保留。

全部 Cargo 已结束，本轮 PostgreSQL 及 Linux V1/V3 holder 均已停止。关闭前其他 DB 连接 0、测试 schema 0，遗留 schema 清理 0 个；用户 5433 未访问。原日志、源码快照与缓存留在 ignored `run/validation-20261012-settings-read-authorization/`，可核对副本与哈希见证据索引。空闲 holder 停止码不是 Cargo 结果。

下一软件候选：先审计 Settings GET/PATCH 共用 update gate 的排队、各阶段预算和取消所有权。current/final-auth 已有 2 秒，gate/preview/apply 等待尚未故障实证；区分准入前等待与 SQL/desired/module 已发生后的部分成功，不直接套整请求 timeout 或全局回滚，不把客户端断开等同 handler 已取消。

未独立动态注入纯更新 gate 排队或 Registry 私有锁等待；最后核验位置由完整 await 静态审查和真实政策等待验证。未运行真实 PTY/Easy-Turn、Linux 全 workspace、release/部署 smoke、外部付费服务、Pi/声学/执行器或长稳。核验是一次响应准入，不承诺后续持续撤权或撤回已发送响应。
