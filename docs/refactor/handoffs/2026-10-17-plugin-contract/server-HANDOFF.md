# 插件契约定稿：Server 所有权与批准边界 HANDOFF

日期：2026-10-17。负责人：`v01_fixture`。只读核对基线 `89d04a2`。本轮用户明确要求保留未来允许安装未经审核第三方代码的能力，并已确认 D01 管理员管理与用户数据隔离、D02 隔离默认及单独明确授予宿主信任的受信本机模式、D03 原期限内排空升级及停用／撤权取消清理。具体隔离后端仍未选择，这些决定不等于允许直接执行当前目录里的代码，或授予外部效果批准。本 agent 仅拥有本文件，不编辑生产、其他交接或统一契约；未运行 Cargo、测试、服务、容器、网络、真实音频或硬件，未读取私有配置。无新增依赖，R03 暂停。

## Checklist

- [x] 复核 Server 当前扩展分类、Bootstrap 接管和与 Skills／MCP 的区别。
- [x] 复核插件管理 HTTP 权限、存储 namespace/scope 与 owner 隔离边界。
- [x] 核对工具实现身份、注册／替换、Execution 提议／批准／claim／收尾路径。
- [x] 向 Root 提交初步目标语义、调用／测试／文档影响。
- [x] 将已实现、源码确认但未动态复现的缺口、未来目标分开记录。
- [x] 根据 Root 转达的用户 D01–D03 决定收敛本文件，并对照统一契约核对；本轮 Server 契约审阅 6/6，不表示生产实现或动态验收已完成。

## 已实现能力及扩展分类

| 类别 | 当前实现 | 信任边界／不能声称的能力 |
| --- | --- | --- |
| Server 内置核心 | Bootstrap、Memory、Execution、Interaction、registries 是宿主拥有的固定服务 | 插件可以扩展受支持的登记表，不能替换政策、工具批准或持久化权威 |
| 编译内工具插件 | `plugins/context.rs:14` 的 `PluginToolContext` 和 `:21` 的 `CompiledToolPlugin`；HA 是当前具体实现 | 与宿主同进程的 Rust 代码，本质是受信代码，不是未审核代码隔离边界 |
| 生命周期 extension | `bootstrap/builder.rs:201` 的 `with_extension`、`:664` 先初始化 extensions 再初始化 skills；通过 Runtime cleanup 接管 | 是已编译 `ModuleLifecycle` 的注册入口，非安装任意包／动态库的第三方安装器 |
| 外部表达插件 | `expression/abi.rs:42` manifest，`:228` 本地目录加载，`:285` JSON stdin/stdout 子进程实现 | 当前仅有环境清理、期限和 kill-on-drop 等进程选项；不是 OS 文件／网络权限沙箱，也不是通用工具插件 SDK。进程细节由本轮另一 agent 专项负责 |
| Skills | `modules/interaction/skills/registry.rs:34` 明确 Skill 不可直接调用，只提供路由／提示与 preferred tools；`:83` 原子提交／索引发布 | 技能文本、目录内容和 tool preference 均不能授予执行批准，不能作为插件实现身份或新信任来源 |
| MCP／动态第三方安装 | 本次有界源码／Cargo 清单未发现 Server MCP host/client adapter 或独立通用 SDK；当前动态生命周期尚未实现，未审核安装／隔离运行已是明确后续目标 | 协议接入属于未来能力；不能把当前工具 trait 或 Embedded PluginManager RPC 等同于 Server 已实现 MCP／第三方运行平台 |

插件专用存储已实现命名空间、collection 声明、scope 类型、schema version、TTL、单条大小限制、CAS 和 uninstall retention（`plugins/storage.rs:65`、`:120`、`:174`、`:819`；`plugins/manifest.rs:36`）。底层 PostgreSQL 是宿主所有，编译内插件只拿 `PluginStorage` trait，不拿 raw DB。该接口有现有 `tests/plugin_storage.rs:100` 真实 PG 用例；本轮未运行或重新认证其结果。

## 上轮发现复核：源码缺口，未动态复现

### S1：管理 HTTP 与跨 owner 存储

`api/plugins.rs:45` list、`:78` usage、`:93` export、`:111` purge 只提取 `AuthUser`，未检查管理员或调用者 owner。`api/mod.rs:49` 直接 merge 插件 routes，无额外管理员 gate；`auth/middleware.rs:26` 的认证会核对账号 active，但不自动提升为管理员授权。

export SQL（`plugins/storage.rs:769`）按 plugin_name 导出全部尚未 TTL 过期的 scope／owner 内容；purge SQL（`:790`）按 plugin 或 collection 删除，不带 actor／owner 条件。因此普通已登录 active 用户可抵达全插件数据管理路径。代码路径已核对，HTTP 动态 RED 尚未执行；用户已确认 D01 将全局／跨用户管理收紧至管理员，普通用户数据走隔离入口。本轮仅定稿契约，尚未修改这些 API。

### S2：存储 namespace 并非每用户访问授权

`PluginStorageHandle` 私有 plugin_name 固定了插件命名空间，但方法接受调用者构造的 `StorageScope::User(String)`／Channel／Conversation（`storage.rs:298`），scope validation 只验证标识格式和 manifest 允许的类型，没有当前执行用户的授权令牌或 owner 解析。它适用于受信编译内插件；不能把同样接口原样暴露给未经审核的子进程，便声称防止跨用户访问。

同理，`PluginToolContext.tools` 是完整 `Arc<dyn ToolRegistry>`；register/unregister 接口的 owner 参数由调用方传入。`tools/builtin.rs:602` 检查 definition.source 与 owner_source 相同，但不是“当前插件身份由宿主不可伪造地绑定”的检查。共享进程受信 Rust 代码本来能任意访问宿主资源，不能依赖此上下文去隔离恶意 Rust 插件。未来隔离扩展必须把 namespace、actor 与权限绑定在宿主代理端。

### S3：现有注册／卸载构件不等于完整插件升级事务

单个工具注册（`tools/builtin.rs:602`）可更新 SQL 元数据、artifact owner，再放入内存 executor；批量插件工具移除（`:713`）会先检查外部依赖并提交删除，再移除内存 executor。它们提供可复用构件，尚未实现“任意第三方包 install→activate→upgrade→rollback→uninstall”的统一持久生命周期。

`PluginStorageService::apply_uninstall_policy` 已能处理 retention，但与进程停止／多个工具、skills、传感器、依赖图和存储的完整卸载顺序没有动态 Server 安装器衔接。此处是明确未实现的能力，不声称已证出现升级数据损坏。后续按已确认 D03 对具体生命周期转换设计失败注入，不先建设通用插件平台。

准确的现有卸载语义（`plugins/storage.rs:819`）是：`force_export_required=false` 时，非空 `export_required` collection 在执行 retention 前阻止卸载；`force_export_required=true` 只跳过这项前置检查，记录仍保留，并计入 `export_required_records`。该参数不等于 purge，也不证明已完成导出。`delete_on_uninstall` 才在该流程删除记录，`retain_on_uninstall` 保留并计数；`purge_plugin` 是独立删除操作，须接受最终管理授权边界约束。后续测试必须区分“跳过检查后保留”和“明确 purge”，不能把强制卸载写成强制擦除。

## 应保留的 Execution 事实与契约

当前调用链：

```text
可信 ingress / Interaction / 自动化
  → ToolContext（宿主分配 user、request、context、source、epoch）
  → ExecutionToolRegistry.execute / HTTP tools.execute
  → ExecutionService.submit
  → ExecutionRepository.propose（SQL 身份／context／目录政策检查）
  → 同 owner 的明确 approve，或满足现有政策时 Automatic approve
  → outbox worker 解析并固定 PreparedTool Arc
  → 复核 implementation/policy/catalog digest、来源和批准期限
  → claim → executor → 成功／失败／Unknown 账本事实 → cleanup
```

精确依据：

- `api/tools.rs:76` 将手动调用送 `state.execution.submit`；`modules/execution/registry.rs:14` 是产品模块唯一可见 wrapper，未开放 raw resolve；注册要求非空 implementation revision（`:84`）。仅 metadata 的工具被 `tools/builtin.rs:595` 拒绝执行。
- `modules/execution/identity.rs:11` 对真实宿主 artifact 计算 build digest；`service.rs:515` 将名称/source/schema、确认和role条件、build digest、implementation revision 与 registration revision 绑定到 tool digest。插件自填同一个版本字符串不能代替未来外部代码包的宿主测量身份。
- `service.rs:234` 有界 submit；`:441` 的 confirm 只由可信认证入口调用；`api/effects.rs:175` 使用 auth.id 和 expected intent digest，不从模型 argument 读取批准。
- `repository.rs:108` 要求批准 actor 与 effect owner 一致，检查当前账号/context/catalog/intent；`:176` 使用数据库时间设原 15 分钟批准期限。管理员安装插件不是批准所有用户的 effect，也不能因管理角色决定就自动改成管理员代替 owner 批准。
- `worker.rs:91` claim 前解析并固定具体 `Arc`；当前实现／政策 digest 不匹配则拒绝未 dispatch 的效果。`repository.rs:248` 同时核对目录政策、实现／政策身份和实际批准期限。已 dispatch 的效果保留原执行器及真实收尾，不能因更新而重试到新执行器。
- 现有取消／关闭保持原 owner、timeout、deadline、Unknown 与不自动重试语义。不可把插件进程结束、HTTP 200、请求取消或安装批准当成外部效果已撤回／成功。

现有可复用测试入口（本轮只读）：`tests/execution_service.rs:642` 模型参数不能自批，`:797` claim 固定旧 executor，`:837` 同 schema 热替换使旧批准失效，`:875` 缺 implementation revision 拒绝，`:909` Unknown 不重试，`:983` 取消 shutdown waiter 保留 join；`tests/server_runtime.rs:918` dispatch 后保留外部插件和数据库至 receipt。后续只补插件组合场景缺口，不重写这些已有保障。

## 已确认目标契约（后续实施）

1. **D01：管理员管理，用户数据隔离。** 全局安装、启用、停止、升级、卸载、权限授予、全量 export/purge 属管理控制面，仅当前 active admin 经身份与授权复验后可操作。普通用户只通过执行批准链使用已允许工具，只看必要使用说明；本人数据入口限定本人／被授权 scope，由宿主绑定身份并检查查询，不复用当前全量 export。管理权限不代替 effect owner 的批准。
2. **D02：隔离默认，受信本机模式单独授权。** 未审核包可安装为未激活资产；安装状态、启用状态、能力授予和外部效果批准分别记录。实际执行默认使用经验证的隔离与宿主能力代理；所需隔离约束不支持时拒绝运行，绝不自动降级。另保留受信本机模式，但必须单独明确授予宿主信任，绑定实际内容身份；它不具有未经审核隔离模式的保证。当前 compiled-in extension 维持受信同进程模型，具体隔离后端及未审核示例均待后续实现与验收。
3. **宿主赋予身份／scope／能力。** 未审核隔离插件的宿主接口不得接受 plugin owner、user ID 或 source metadata 来取得权限，也不得交出 raw DB、通用 ToolRegistry、全部文件系统、任意网络或宿主凭据。只从当前已授权调用获得宿主绑定的能力；其输出是数据，不是自动可信 Memory/Appraisal/执行来源。受信本机代码实际拥有宿主账户权限，不声称此接口约束能够强制隔离恶意受信代码。具体文件／网络／凭据能力随真实首个扩展用例限定，不发明本轮全套新框架。
4. **批准绑定实际内容和权限。** 延续现有 Execution digest 与原政策；未来外部执行器身份至少由宿主确认的实际版本／内容、目标配置和权限构成。包升级或权限扩张不得复用旧审批。仍允许不同信任来源的插件，但未经审核代码不能自己签发可信 pin 或绕过 current policy。
5. **D03：排空后升级，停用／撤权取消并清理。** 升级关闭新 admission，旧调用保留原实例与原期限，实际排空并完成资源清理后才切换新版；不延长期限，不让新版本继承旧执行批准。停用／撤权关闭准入并取消相关调用，保留原 owner 等待真实清理。清理预算耗尽时明确未完成并保留 owner／资产引用，不伪装 Stopped、升级成功或回滚成功。已 dispatch effects 的迟到真实回执、失败及 Unknown 事实按原身份保留，不恢复授权、不自动重试；uninstall 顺序复用依赖图、资源 owner、retention，不直接删除未知结果记录。

## 调用、测试与文档影响

| 后续工作 | 最小受影响位置 | 可独立软件验证的断言 |
| --- | --- | --- |
| 管理权限收紧 | `api/plugins.rs`、必要的鉴权／存储 repository helper、HTTP 测试 | admin允许；普通用户／降权／inactive／deleted拒绝；被拒绝不导出原值、不删除任何scope；等待后身份按所选 admission 规则复验 |
| 用户 scope／未来隔离 storage broker | `plugins/context.rs`、`plugins/storage.rs` 的调用包装层；保持原表与CAS语义可复用 | 插件 namespace 不可伪造；user/channel/conversation owner 由宿主绑定；跨owner和未声明collection拒绝；TTL/CAS/retention保持 |
| 外部工具接入批准链 | `providers/adapter.rs`、`modules/execution/{registry,service,worker}.rs`、插件接入 adapter | 不能以模型参数批准；内容／权限改变后旧approval不dispatch；重复request不重复效果；Unknown不重试；旧in-flight不误调用新版本 |
| 生命周期 | Bootstrap／Runtime cleanup、tool/skill/artifact registry、plugin storage retention | 启动部分失败回收；同名冲突不夺取别的owner；停止屏蔽新调用；取消等待仍可重新join；依赖或export_required阻止卸载；失败状态可恢复 |
| SDK／验收资料 | `docs/concepts/modules-and-providers.md`、`packages/alia-server/docs/plugin-storage.md`、最终插件契约、一个有界示例 | 明确受信compiled／隔离未审查代码／Skill／外部服务协议区别；列实际host API、版本、禁止权威与证据，不把MCP或安装器写成已完成 |

建议管理权限先保存确定性 HTTP RED，再做最小修复；使用隔离 PG、普通用户和管理员、至少两 owner 的同插件存储，先证被拒绝时数据和资源未动。进程／ABI 场景由本轮对应 agent 负责，本文件不重复扩张其实现计划。后续无需真实 Home Assistant、模型、硬件或外部网络，用本地受控 executor／stub 完成组合验收即可。

## 统一契约交叉核对

已对照 `docs/architecture/plugin-contract.md` 完成本 agent 所负责的 Server 分类、管理／存储边界、工具身份及 Execution 批准路径核对，并按 Root 转达的用户答复落实 D01–D03。契约将当前能力与目标要求分开，并保留宿主核心权威、现有批准账本、实际内容身份、原 in-flight owner 和资源收尾，没有将现有编译内接口冒称为未经审核代码的隔离框架。以上核对仅为源码与文档一致性审阅，不是动态验收。

D01–D03 均已确认；已逐项保留管理员管理与用户数据隔离、隔离默认与受信本机模式单独明确授予、无自动降级、原期限排空并清理后升级、停用／撤权取消清理以及不继承旧执行批准。插件管理授权仍不代替 effect owner 的现有批准；`force_export_required` 仍不是删除授权或已完成导出的证据。本轮 Server 契约审阅完成 6/6；所列静态缺口、隔离后端选择及具体实现和动态验收仍是后续工作。

## 交接停止条件

- 当前未编写生产补丁、测试或新的协议框架，所有缺口仍保持“源码确认／尚未动态复现”标签。
- 用户已确认管理角色、隔离默认与受信模式、升级／撤权语义；本轮将决定收敛为目标契约，不据此启动外部代码、服务或硬件。
- 用户保留未来未审核第三方代码能力的要求已经纳入；不以默认拒绝所有第三方代码作为最终设计，也不以当前无隔离实现直接执行来冒充完成。
- 本 agent 的有界源码／统一契约审阅及决定收敛完成 6/6，本文件冻结。本轮文档由 Root 正常提交；具体生产实现、RED/GREEN 和动态证据归档属于后续授权任务。
