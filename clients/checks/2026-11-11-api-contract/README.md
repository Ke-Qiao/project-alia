# API 契约与错误模型 — 2026-11-11

起点是干净的 `a2d5b092d6ae`（`codex/web-account-session`），本轮在 `codex/api-contract-errors` 施工。按用户要求推送的是原 Web 会话分支，并用 [`git ls-remote`](logs/preplan-verify-20261110T203904959174Z.log) 验证远端与起点提交一致。本轮重构仅本地提交，未推送。使用 [Node 24.15.0、pnpm 10.33.4 和 cargo 1.96.0](logs/00-tool-versions-20261110T203417772346Z.log)，未升级依赖。

命令完整输出和退出码保存在 [`logs/`](logs/)，本机原件位于 `run/api-contract-2026-11-11/`。失败和中间检查也保留。

## 做了什么

- 服务端通用错误保留 `code` 和 `message`，新增可选 `reason`。账户验证超时返回 503 `SERVICE_UNAVAILABLE`，重复邮箱返回 409 `ALREADY_EXISTS`。移除旧的 `Auth(String)` 和 Companion 重复声明的六个通用错误；三个带操作 ID 或修订号的 Companion 回执及 Execution、Delivery 域错误保留。
- OpenAPI 声明通用错误及需要登录的操作的 401/503，消除重复 `operationId`，补齐 logout、`PATCH /users/me` 和六个会话操作。导出 spec 后由工具生成 TypeScript SDK。143 个操作中 47 个已声明，96 个待补，详见 [OpenAPI 覆盖表](../../../packages/alia-server/docs/openapi-coverage.md)。
- Web 注册按 409 和 `email_already_registered` 给邮箱字段报错。登出先清本地会话，再用生成的 `logout` 通知服务端；失败不重试、不阻塞。`useMutation(logout)` 通过原 Tayori Provider 注入客户端，保留 API 地址、Mock 和拦截器。仅刷新返回 401 才结束会话，503 仍保留会话。

## 检查结果

| 检查 | 结果 | 日志 |
| --- | --- | --- |
| `cargo fmt --all -- --check` | 通过 | [格式](logs/AB-format-final-20261110T204006162818Z.log) |
| `cargo clippy -p alia-server --all-targets` | 退出 0；保留已有警告 | [Clippy](logs/AB-clippy-final-20261110T204504472749Z.log) |
| `cargo test -p alia-server --lib error::tests` | 1 个表驱动测试通过，覆盖 12 个非认证变体（含 gRPC）和 5 种认证失败；检查 status、code、reason、message 和私密诊断不外泄 | [错误体](logs/A-error-final-20261110T203959439903Z.log) |
| `cargo test -p alia-server --lib openapi::tests` | 5 个通过，含原有 Execution 和 Companion 契约检查 | [契约](logs/AB-contract-final-20261110T204523472073Z.log) |
| 指南的 `--lib error` / `--lib auth` | `error` 匹配了其他模块：14 个通过，7 个因沙箱禁止本地端口失败，2 个缺少临时 PostgreSQL；`auth` 只列出命中的 16 个用例，未再次执行不相关的测试 | [error 首轮](logs/A-error-20261110T203617569542Z.log)、[auth 清单](logs/A-auth-inventory-20261110T204007254254Z.log) |
| 完整 Rust 测试 | 未运行；`ALIA_TEST_DATABASE_URL` 未设置，未启动数据库 | [环境记录](logs/00-preflight.log) |
| `pnpm web:openapi` | 导出 spec 并生成 SDK，退出 0 | [生成](logs/C-openapi-final-20261110T204437124160Z.log) |
| 导出契约盘点 | 通过：39→47 操作，47 个唯一 ID，42 个需要登录的操作有 JSON 401/503，原有成功响应及递归引用的 47 个 schema 不变 | [spec 核对](logs/B-spec-final-20261110T204517360801Z.log) |
| 路由覆盖盘点 | 通过：25 个 `api/` 文件，47 已声明、96 待补；`GET /health` 在根路径 | [路由核对](logs/B-coverage-audit-20261110T204122198368Z.log) |
| `pnpm web:lint` | 通过，0 问题 | [lint](logs/C-lint-final-20261110T204353339223Z.log) |
| `pnpm web:typecheck` | 通过；直接 `useClient()` 的类型过宽，改用现有 `useMutation` 解决 | [最终](logs/C-typecheck-final-20261110T204353339543Z.log)、[初次失败](logs/C-typecheck-20261110T204122198384Z.log) |
| `pnpm web:test` | 16 个通过 | [Web 测试](logs/C-web-test-final-20261110T203845642552Z.log) |
| `pnpm web:build` | 通过：`/`、`/account`、`/login`、`/register` 已静态生成。首轮沙箱拒绝 Turbopack 绑定内部端口，使用干净缓存重跑通过 | [最终构建](logs/C-build-clean-cache-20261110T204603189661Z.log)、[首轮](logs/C-build-20261110T204122198364Z.log) |
| 浏览器手工 Mock | 通过：重复邮箱显示字段错误；用 Mock 成员登录后可打开账号页；退出回到登录页。专用 Web 预览已停止 | [浏览器记录](logs/C-browser.log)、[预览启停](logs/C-preview-server-20261110T204800304685Z.log)、[重复邮箱](logs/duplicate-email.jpg)、[账号页](logs/mock-account.jpg)、[退出后](logs/logged-out.jpg) |
| 真实后端联调 | 未验证；未启动后端或连接设备 | — |

## 指南验收表

| 项 | 结论 | 证据 |
| --- | --- | --- |
| 1. 除指定两处外，状态码和 `code` 不变 | 是 | 修改的错误转换经源码对照；[错误体](logs/A-error-final-20261110T203959439903Z.log)及[域错误测试](logs/AB-contract-final-20261110T204523472073Z.log)通过。此项未做真实后端逐路由回放。 |
| 2. 认证失败有独立 `reason` 并经测试覆盖 | 是 | [错误体测试](logs/A-error-final-20261110T203959439903Z.log)覆盖五种 `AuthFailure`。停用账户登录继续用统一的凭据错误。 |
| 3. 鉴权查库超时 503；重复邮箱 409 `ALREADY_EXISTS` | 是 | 源码转换和[错误体测试](logs/A-error-final-20261110T203959439903Z.log)；未连真实 PostgreSQL 制造超时或重复写入。 |
| 4. 通用错误 schema 和鉴权 401/503 已声明 | 是 | [导出核对](logs/B-spec-final-20261110T204517360801Z.log)。 |
| 5. `operationId` 唯一，有回退检查 | 是 | [契约测试](logs/AB-contract-final-20261110T204523472073Z.log)。 |
| 6. 本批八个操作全已声明 | 是 | [导出核对](logs/B-spec-final-20261110T204517360801Z.log)。 |
| 7. 原有 Execution 和 Companion 契约测试保留并通过 | 是 | [契约测试](logs/AB-contract-final-20261110T204523472073Z.log)。 |
| 8. Web 不再匹配报错文本，登出会请求服务端 | 是 | 源码核对、[Mock 会话测试](logs/C-web-test-final-20261110T203845642552Z.log)覆盖请求和旧 token 失效，[手工 Mock](logs/C-browser.log)确认字段提示和退出跳转。 |
| 9. 四条 Web 命令均通过 | 是 | [lint](logs/C-lint-final-20261110T204353339223Z.log)、[typecheck](logs/C-typecheck-final-20261110T204353339543Z.log)、[test](logs/C-web-test-final-20261110T203845642552Z.log)、[build](logs/C-build-clean-cache-20261110T204603189661Z.log)。 |
| 10. 真实后端联调 | 未验证 | 本轮运行范围是本地单元、生成、静态核对和 Mock。 |

## 后续工作与实施选择

- 继续补剩余 96 个操作，并核对已声明操作的业务错误；例如共享账户查询还可能返回 500，自动补 401/503 不代表所有响应都已声明。
- [API 设计待考察事项](../../../packages/alia-server/docs/api-design-follow-ups.md) 记录了登出吊销范围、请求提取错误和生成器类型、域错误分类、会话存在性、管理员自改权限、历史分页六项。本轮没有改变这些策略。
- 项目尚未公开，按用户要求直接切换到最终错误模型，未保留旧错误类型或中间兼容提交。测试使用实际错误回执和最终 OpenAPI 形状。
