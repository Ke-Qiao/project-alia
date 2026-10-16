# CompanionInvitation 历史只读引用

2026-10-16；本轮契约，实施和验证状态见 [checklist](checklists/invitation-history-2026-10-16.md)。用户已确认：已提交的主动邀请是历史事实，输出授权单独失效。默认 shadow 不变。

## 直接读取与授权分离

普通 Generation 可以读取同 owner、session、channel 中已经原子提交的邀请 assistant 文本。必须匹配规范 Response、原邀请 Context、不可变 admission 身份及内容摘要；pending、无来源或正文变化的行不能进入模型历史。新的 Generation 来自本次真实用户消息，使用自己的 request 和当前政策，不沿用邀请的主动输出授权。

历史采用专用 `CompanionHistory` 证据，保存 Message、Response、Context 和正文身份，并合并原邀请全部非 `CompanionAdmission` 来源。不得放宽通用 parent purpose 校验，也不得把 admission 或 private grant 混入 Generation。原邀请仍只授权原有 ResponseCommit / OutputAdmission 两项用途；历史引用不会重播、续期、重新批准工具或产生新的主动表达权限。已提交文本不证明用户已听到。

| 变化 | 历史读取 | 原输出权限 |
| --- | --- | --- |
| 邀请输出期限到期 | 原来源仍有效时可读 | 按原期限失效 |
| Server 重建服务或重启退休 grant | 持久身份与来源仍有效时可读 | 不恢复旧输出 |
| 主动模式改回 shadow | 不以 live 开关作为历史门槛 | 保持 shadow 禁止主动发送 |
| 原始来源删除、正文变更、隐私 epoch 或当前 Context 政策变化 | 省略新的历史候选；旧派生使用再校验失败 | 继续沿用原撤回与精确取消 |
| Appraisal / Memory 等根自身过期 | 不可引用 | 不因历史读取延长 |

读取使用既有历史条数、证据数量和字节上限。一个候选失效或超限时整体省略，数据库错误仍报告失败，不能只保留正文并丢掉来源。历史文本不取得新的来源接纳时间。

## 派生与撤回

新 Generation 进入既有 Response、Cache、Memory、Appraisal、Execution 和输出接纳流程，每个用途继续在事务中验证当前政策与完整来源。Memory 的持久根必须保留只读历史 marker 及非 admission 闭包。直接历史读取要求同会话；既有受控 Memory 等派生的根验证可以使用 synthetic Context，不把邀请会话误当作每次派生的当前会话。

携带该历史根的 Appraisal 不能再成为 Companion 来源，防止主动表达经普通对话反复强化自身。该限制也适用于间接通过 Memory 或 Cache 传递的根。因此，当前普通对话若持续继承邀请历史，其新 Appraisal 也不能再次触发邀请；本轮不把“当前用户又说了一句话”当作删除实际历史依赖的理由。若后续需要此类新输入恢复主动候选，必须另行定义并验证 Appraisal 实际读集的独立来源契约，不能简单白名单放行 marker。

本轮保证来源失效后不能继续通过模型引用与派生授权。显式 privacy erase 和 relationship clear 继续执行现有持久擦除链。裸删除或修改 Message / Response 不是“物理擦掉全部后代”的 API：现有管理视图中的已存 Memory、会话展示及审计记录不会因此一律删除。该既有展示/擦除边界不能与模型再利用权限混淆，本轮不扩展为全局物理擦除框架。

## 本轮交付边界

三个 subagent 分别负责生产契约、确定性回归、独立审查，Root 统一运行与提交。先保存旧实现 RED，再验证最终源码。原期限、默认并发、权限与执行批准不变；无新依赖、迁移或硬件动作。R03 保持暂停；历史 V01 根因独立保留。

回归、资源清理、源码身份和剩余限制见 [四份 HANDOFF](handoffs/2026-10-16-invitation-history/README.md) 与 [恢复检查点](resume-checkpoint-invitation-history-2026-10-16.md)。
