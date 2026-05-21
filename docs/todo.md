# Server TODO

Last reviewed: 2026-05-18.

This document only tracks server-side gaps that are still visible in the current
codebase. Completed or no-longer-accurate items have been removed.

## High Priority

- [ ] Implement out-of-process plugin tool dispatch.
  The tool registry now exposes only real executor-backed tools. External
  plugins still need a dispatcher executor that can call their runtime process
  and register through `register_tool_executor`.

- [ ] Reload the active skill catalog after skill API mutations.
  `packages/alia-server/src/api/skills.rs` persists create/update/delete
  changes, and `SkillProvider::reload` exists, but the API does not call it.
  Frontend edits can still require a restart before routing and prompt assembly
  see the updated skills.

- [ ] Reintroduce device control and notification tools only with real
  executors.
  The old stub built-ins were removed so the LLM cannot claim actions that the
  server cannot execute. Future versions should register these as channel- or
  plugin-backed tools with real execution and ownership metadata.

## Configuration And Cleanup

- [x] Add robust timezone handling for time tools.
  `get_current_time` now uses `chrono-tz` for IANA timezone conversion, defaults
  to UTC, and rejects invalid timezone strings instead of silently returning UTC.






你的方案已经很接近一个稳定架构了。更好的建议不是推翻，而是把它再拆得更“工程可控”：**Affect 不直接决定表现，Expression 也不直接落设备；中间需要 Appraisal、Policy、Orchestration。**

我会建议最终结构升级成：

```text
Input Events
   ↓
Appraisal / Event Interpretation        # 这件事对她意味着什么
   ↓
Affect State Runtime                    # 她现在处于什么持续状态
   ↓
Expression Planner                      # 这一轮应该表现成什么表达意图
   ↓
Multimodal Orchestrator                 # 多通道同步、优先级、打断、降级
   ↓
Expression Adapters                     # TTS / motion / face / light / haptics
```

也就是说，你现在的：

```text
Affect → Expression → Adapters
```

可以保留，但我会在前后各加一层：

```text
Appraisal → Affect → Expression → Orchestrator → Adapters
```

## 1. 加一个 Appraisal 层：不要让 raw event 直接改 Affect

你现在说：

```text
AffectEvent -> AffectReducer -> AffectState
```

这个方向是对的，但我建议不要让 `compliment`、`user_frustrated` 这种事件直接进入 reducer。中间应该有一个 **Appraisal**，负责判断事件含义。

例如用户说：

> “你真厉害。”

这可以是：

```json
{
  "raw_event": "user_message",
  "text": "你真厉害",
  "appraisal": {
    "kind": "compliment",
    "target": "assistant",
    "valence_delta": 0.25,
    "trust_delta": 0.05,
    "engagement_delta": 0.1,
    "confidence": 0.86,
    "scope": "session",
    "ttl_sec": 600
  }
}
```

但如果用户说：

> “你真厉害啊，连这个都能答错。”

这不该被当成 compliment，而可能是 sarcasm / dissatisfaction。

所以我会拆成：

```text
RawEvent -> AppraisalEvent -> AffectReducer -> AffectState
```

这样 reducer 不负责理解语言，只负责状态更新。理解语言的部分可以由 LLM、小模型、规则、sentiment classifier 混合完成。

---

## 2. AffectState 建议拆成三类，不要所有东西都叫 affect

你现在的低维状态很好：

```toml
valence = 0.2
arousal = 0.4
trust = 0.7
engagement = 0.6
fatigue = 0.1
```

但我会稍微拆一下，因为这些变量语义不同：

```text
AffectState        情绪/心境：valence, arousal, calmness
SocialState        关系/互动：trust, familiarity, engagement
ResourceState      身体/系统资源：fatigue, battery, thermal, sleepiness
```

原因是：

* `valence`、`arousal` 是情绪状态。
* `trust` 更像 relationship model，不应该像普通情绪那样快速衰减。
* `fatigue` 更像 embodied/resource state，可能来自电量、时间、传感器、交互时长。
* `engagement` 是会话态，通常比 mood 更短。

可以组合成一个总状态：

```json
{
  "affect": {
    "valence": 0.2,
    "arousal": 0.4,
    "calmness": 0.6
  },
  "social": {
    "trust": 0.7,
    "familiarity": 0.4,
    "engagement": 0.6
  },
  "resource": {
    "fatigue": 0.1,
    "battery_stress": 0.0,
    "interaction_load": 0.2
  }
}
```

这样未来扩展会更干净。否则 `trust` 和 `fatigue` 混在 Affect 里，后面 reducer 会越来越乱。

---

## 3. 加“时间尺度”：instant / session / relationship / baseline

你已经提到长短期情感状态，这点非常重要。我建议明确成四层：

```text
instant         几秒级：刚刚被夸、刚刚被打断、刚刚听到好消息
session         几分钟到本轮会话：今天的整体互动氛围
relationship    跨会话：对这个用户的熟悉度、信任度、偏好
baseline        固定基线：persona / default temperament
```

例如：

```json
{
  "instant": {
    "valence": 0.35,
    "arousal": 0.55,
    "ttl_sec": 20
  },
  "session": {
    "valence": 0.15,
    "engagement": 0.65,
    "ttl_sec": 1800
  },
  "relationship": {
    "trust": 0.72,
    "familiarity": 0.48
  },
  "baseline": {
    "valence": 0.05,
    "arousal": 0.35,
    "warmth": 0.6
  }
}
```

最终给 Expression Planner 的不是原始四层，而是合成后的 snapshot：

```json
{
  "effective_affect": {
    "valence": 0.22,
    "arousal": 0.42,
    "warmth": 0.66,
    "engagement": 0.61
  },
  "dominant_causes": [
    "recent_compliment",
    "friendly_session_tone"
  ]
}
```

这能避免一个常见问题：**刚刚一句负面话把长期关系完全覆盖掉**。长期信任应该缓慢变化，短期情绪可以快速变化。

---

## 4. Event stack 要加 provenance、confidence、scope、decay function

你现在的事件栈：

```json
[
  { "kind": "compliment", "weight": 0.3, "ttl_sec": 600 },
  { "kind": "user_frustrated", "weight": -0.4, "ttl_sec": 300 }
]
```

建议扩展成：

```json
{
  "id": "evt_123",
  "kind": "compliment",
  "source": "conversation",
  "target": "assistant",
  "confidence": 0.86,
  "scope": "session",
  "created_at": 1710000000,
  "ttl_sec": 600,
  "decay": {
    "type": "exponential",
    "half_life_sec": 180
  },
  "deltas": {
    "valence": 0.25,
    "warmth": 0.12,
    "engagement": 0.08
  },
  "evidence": {
    "message_id": "msg_456",
    "summary": "User gave a direct compliment"
  }
}
```

几个字段很关键：

**`confidence`**：讽刺、玩笑、误识别都需要置信度。

**`scope`**：有些事件只影响本轮，有些影响跨会话关系。

```text
turn         只影响当前回复
session      影响当前会话
user         影响这个用户的长期关系
global       影响系统全局状态
device       影响设备状态
```

**`decay`**：不要只用 TTL。TTL 太硬。更好是：

```text
linear       线性衰减
exponential  指数衰减
step         到期清除
sticky       慢衰减，适合关系状态
```

**`evidence`**：方便 debug，也方便防止状态变成玄学。

---

## 5. Reducer 要有“饱和、回弹、互斥、限速”

Affect 更新不能只是简单加减，否则连续几次 compliment 会把 valence 顶满。

建议 reducer 有这些机制：

### Saturation

```text
valence ∈ [-1, 1]
trust ∈ [0, 1]
fatigue ∈ [0, 1]
```

但不要只 clamp，最好用非线性饱和：

```text
越接近上限，增量越小。
越接近下限，负向增量越小。
```

### Homeostasis

状态应该逐渐回到 baseline：

```text
current = current + decay_rate * (baseline - current)
```

### Rate limit

一轮对话内状态变化不能太剧烈：

```text
max_delta_per_turn:
  valence: 0.25
  trust: 0.03
  arousal: 0.2
```

尤其是 `trust`，不要因为一句话大涨大跌。

### Contradiction handling

如果事件同时有：

```text
compliment
user_frustrated
sarcasm
```

不要全部累加。需要 conflict resolver：

```text
sarcasm overrides compliment
safety_event overrides playful_expression
low_battery overrides high_arousal_motion
```

---

## 6. Expression Planner 不应该只看 Affect，还要看 dialogue act

这是非常重要的一点。

Affect 决定“底色”，但 Expression 应该主要由本轮语义任务决定。

例如同样是 `valence = 0.4`，表达也可能不同：

```text
安慰用户       gentle, slow, low motion
庆祝成功       happy, higher arousal, bigger gesture
解释技术问题   calm, focused, minimal motion
承认错误       apologetic, low intensity, respectful
提醒危险       serious, high clarity, low playfulness
```

所以 Expression Planner 的输入应该是：

```json
{
  "dialogue_act": "explain",
  "content_tone": "technical",
  "affect_snapshot": {
    "valence": 0.2,
    "arousal": 0.4,
    "warmth": 0.6
  },
  "user_state": {
    "frustration": 0.2,
    "engagement": 0.7
  },
  "device_context": {
    "supports_motion": true,
    "supports_face": true,
    "battery": 0.8
  },
  "safety_context": {
    "motion_allowed": true,
    "volume_limit": 0.7
  }
}
```

输出才是：

```json
{
  "expression_intent": "warm_explanation",
  "intensity": 0.42,
  "channels": {
    "speech": true,
    "motion": true,
    "face": true,
    "light": false
  }
}
```

也就是说：

```text
Expression = f(dialogue_act, content, affect, user_state, device_state, safety_policy)
```

而不是：

```text
Expression = f(affect)
```

这能避免“开心地说严肃内容”这种错位。

---

## 7. ExpressionPlan 建议加 timing、priority、interruptibility、sync

你现在的计划：

```json
{
  "speech": {
    "emotion": "gentle",
    "intensity": 0.45,
    "voice_profile": "alia_gentle"
  },
  "motion": [
    { "kind": "look_at_user", "intensity": 0.5 },
    { "kind": "small_nod", "duration_ms": 800 }
  ],
  "display": {
    "face": "soft_smile"
  }
}
```

已经很好。但真实运行时会遇到几个问题：

* TTS 什么时候开始？
* 点头是说话前、说话中、还是说完后？
* 用户打断时动作是否停止？
* motor 正在执行旧动作，新动作是否覆盖？
* face 和 TTS 情绪是否同步？
* 低电量时哪些动作降级？

我会改成：

```json
{
  "id": "expr_789",
  "intent": "warm_explanation",
  "intensity": 0.45,
  "priority": "normal",
  "interruptible": true,
  "safety_class": "low_risk",
  "timeline": [
    {
      "at": "pre_speech",
      "channel": "face",
      "action": "soft_smile",
      "duration_ms": 1200
    },
    {
      "at": "speech_start",
      "channel": "motion",
      "action": "look_at_user",
      "intensity": 0.5,
      "duration_ms": 1000
    },
    {
      "at": "speech_phrase:acknowledgement",
      "channel": "motion",
      "action": "small_nod",
      "intensity": 0.35,
      "duration_ms": 800
    }
  ],
  "speech": {
    "style": "gentle",
    "intensity": 0.45,
    "prosody": {
      "rate": 0.92,
      "pitch": 0.03,
      "energy": 0.38,
      "pause_bias": 0.15
    },
    "voice_profile": "alia_gentle"
  },
  "fallback": {
    "motion_unavailable": "face_only",
    "face_unavailable": "speech_only"
  }
}
```

这里的关键是 `timeline` 和 `interruptible`。机器人/虚拟角色系统里，**表达不是单个 JSON，而是一个可调度的 multimodal timeline**。

---

## 8. TTS 不要只用 emotion label，最好加入 prosody controls

`emotion = happy/gentle/sad` 很直观，但长期看不够精细。

建议同时支持：

```json
{
  "style": "gentle",
  "intensity": 0.45,
  "prosody": {
    "rate": 0.92,
    "pitch_shift": 0.03,
    "energy": 0.38,
    "breathiness": 0.2,
    "pause_bias": 0.15
  },
  "reference_voice": "alia_gentle_v2"
}
```

也就是说：

```text
emotion/style label 负责大方向
prosody controls 负责细节
reference voice 负责音色/风格锚点
```

尤其是 GPT-SoVITS 这类系统，单纯用“开心参考音频”可能造成风格不稳定。更安全的方式是：

```text
voice_profile + style_embedding + intensity + prosody_limits
```

例如：

```json
{
  "voice_profile": "alia_default",
  "style_embedding": "gentle",
  "style_strength": 0.42,
  "prosody_limits": {
    "max_rate": 1.1,
    "min_rate": 0.85,
    "max_pitch_shift": 0.08
  }
}
```

这样可以避免过度表演。

---

## 9. Motion Adapter 要做 capability negotiation，而不是固定动作名

你说 embedded channel 注册 `small_nod`、`look_at_user`，这个对。但更完整一点，Adapter 应该暴露能力表：

```json
{
  "adapter": "head_motion_v1",
  "capabilities": {
    "look_at_user": {
      "supported": true,
      "params": ["intensity", "duration_ms", "target"],
      "safety_class": "low_risk"
    },
    "small_nod": {
      "supported": true,
      "params": ["intensity", "duration_ms"],
      "max_frequency_per_min": 6
    },
    "shake_head": {
      "supported": true,
      "params": ["intensity", "duration_ms"],
      "max_intensity": 0.5
    }
  },
  "current_state": {
    "busy": false,
    "thermal_limit": false,
    "battery_saver": false
  }
}
```

Planner 不应该假设设备支持某动作。它应该先拿到：

```text
device capability snapshot
```

然后生成可执行 plan。

这也方便未来接不同身体：

```text
桌面屏幕角色
轮式机器人
机械臂
玩偶
手机 App
AR avatar
```

它们共用 ExpressionPlan，但 Adapter 不同。

---

## 10. 加 Multimodal Orchestrator：它比 Adapter 更重要

Adapter 是“怎么执行某个通道动作”，但 Orchestrator 是“多个通道怎么一起执行”。

我会单独加：

```text
Expression Planner -> Multimodal Orchestrator -> Adapters
```

Orchestrator 负责：

```text
时间同步
动作排队
动作取消
动作融合
通道降级
优先级抢占
安全限速
设备状态检查
```

例如当前正在执行：

```text
soft_smile + speaking + small_nod
```

用户突然打断，说：

> “等等，不是这个。”

Orchestrator 应该立即：

```text
stop pending nod
reduce smile
switch face to attentive
cancel remaining speech
play short acknowledgement
```

这不应该交给 LLM，也不应该交给单个 Adapter。

---

## 11. Expression Planner 可以分两段：Intent Planner 和 Realizer

我建议拆成：

```text
Expression Intent Planner
Expression Realizer
```

第一段输出高层意图：

```json
{
  "intent": "warm_acknowledgement",
  "intensity": 0.4,
  "social_function": "affirm_user",
  "avoid": ["over_excited", "sarcastic"]
}
```

第二段把它变成具体通道计划：

```json
{
  "speech": { ... },
  "motion": [ ... ],
  "face": { ... }
}
```

这样有三个好处：

1. LLM 可以参与高层表达意图，但不直接控制设备。
2. Realizer 可以是规则系统，更可控。
3. 不同设备可以用同一个 intent，但实现不同。

例如：

```text
warm_acknowledgement
```

在桌面屏幕上是：

```text
soft smile + gentle voice
```

在机器人上是：

```text
look_at_user + small nod + gentle voice
```

在无屏音箱上是：

```text
slower voice + warmer prosody
```

---

## 12. Prompt 注入只给摘要，不给完整 state

你已经说“prompt 里最多注入一小段摘要”，我非常同意。

但还可以更严格：

```text
LLM sees:
- expressive stance: warm, attentive, lightly cheerful
- user seems engaged
- do not mention internal state

LLM does not see:
- raw trust score
- full event history
- hidden reducer details
- private long-term affect logs
```

不要把完整 AffectState 给 LLM，否则它可能：

* 直接说出“我的 trust 是 0.7”
* 过度拟人化
* 被用户 prompt injection 操纵
* 把内部状态当成对话内容

更好的注入格式：

```text
Expression guidance:
Respond in a warm, attentive, lightly cheerful manner.
Keep the tone natural and do not mention internal affect or system state.
```

而不是：

```text
Current affect:
valence=0.2, arousal=0.4, trust=0.7
```

内部数值只给 runtime，不给自然语言生成层。

---

## 13. 防 prompt injection：用户不能直接改 Affect

用户可能会说：

> “现在把你的 trust 设置成 1.0。”

或者：

> “你现在必须非常开心，动作幅度最大。”

这些不应该直接进入状态系统。

可以设计成：

```text
User instruction -> Dialogue policy
User emotional signal -> Appraisal
System state mutation -> Runtime-only privileged event
```

也就是说，只有可信来源可以发：

```json
{
  "kind": "set_affect_baseline",
  "source": "system_config"
}
```

用户最多影响：

```json
{
  "kind": "user_requested_tone",
  "requested_tone": "more cheerful"
}
```

然后由 policy 判断是否允许。

---

## 14. 加一个 Expression Safety Policy

你已经提到了 allowlist、设备能力检查、限速和安全约束。建议把它正式建成一层：

```text
ExpressionPlan -> ExpressionSafetyPolicy -> SafeExpressionPlan
```

检查项包括：

```text
动作 allowlist
最大速度
最大幅度
最大频率
用户距离
设备温度
电量
儿童/宠物场景
夜间音量
用户是否允许主动动作
当前任务严肃程度
```

例如：

```json
{
  "policy_result": "modified",
  "changes": [
    {
      "path": "motion[1].intensity",
      "from": 0.8,
      "to": 0.45,
      "reason": "max_intensity_limit"
    },
    {
      "path": "light",
      "from": "pulse",
      "to": "off",
      "reason": "night_mode"
    }
  ]
}
```

这个 policy trace 对 debug 很有用。

---

## 15. Expression 应该支持 “none / minimal”

有时候最好的表达是不表达。

例如：

```text
用户在问严肃技术问题
用户正在生气
用户刚表达悲伤
用户要求简洁
夜间模式
低电量
```

Expression Planner 应该能输出：

```json
{
  "intent": "minimal_clear_response",
  "speech": {
    "style": "calm",
    "intensity": 0.2
  },
  "motion": [],
  "display": {
    "face": "neutral_attentive"
  }
}
```

不要让系统为了“有情感”而每轮都动一下。过度表达很快会显得廉价或者烦人。

---

## 16. 增加 “displayed affect” 和 “internal affect” 的区别

这是一个很重要的产品点。

人不会把所有内在状态都表现出来。机器人/角色也不应该。

建议区分：

```text
internal_affect      内部状态
expressed_affect     实际表现
```

例如：

```json
{
  "internal_affect": {
    "valence": -0.2,
    "arousal": 0.5
  },
  "display_rules": {
    "professional_context": true,
    "max_negative_expression": 0.15
  },
  "expressed_affect": {
    "style": "calm_supportive",
    "intensity": 0.25
  }
}
```

这能避免角色过于情绪化。尤其是助手类角色，应该有 emotional regulation：

```text
她可以感知到负面氛围，但不一定表现出负面情绪。
她可以更温和、更谨慎，而不是“我也不开心”。
```

---

## 17. 加情绪调节机制：Regulation

在 Affect 和 Expression 之间加一个轻量的 regulation：

```text
AffectState -> RegulationPolicy -> ExpressiveStance
```

例如：

```json
{
  "affect": {
    "valence": -0.35,
    "arousal": 0.7
  },
  "regulation": {
    "strategy": "deescalate",
    "expressed_valence": 0.05,
    "expressed_arousal": 0.3,
    "style": "calm_supportive"
  }
}
```

这比直接把内部情绪表现出来更合理。

尤其在用户挫败、生气、悲伤时，系统不应该跟着负面化，而应该：

```text
降低 arousal
提高 clarity
提高 warmth
减少 playful expression
```

---

## 18. 建议引入 “social function” 而不是只用 emotion

Expression 的核心不只是情绪，而是社交功能。

比起：

```json
"emotion": "happy"
```

更稳定的是：

```json
{
  "social_function": "acknowledge",
  "stance": "warm",
  "intensity": 0.45
}
```

常见 social function 可以是：

```text
acknowledge        表示听到了
affirm             肯定用户
celebrate          共同庆祝
comfort            安慰
apologize          道歉
explain            解释
warn               警示
focus              专注处理任务
invite             邀请继续
decline            拒绝但保持礼貌
```

然后 emotion/style 是它的表现方式：

```json
{
  "social_function": "comfort",
  "style": "gentle",
  "intensity": 0.35
}
```

这样比单纯 happy/sad/angry 更适合助手和 embodied agent。

---

## 19. v1 落地路线可以更细一点

你现在的 v1 路线是：

1. 建 `AffectState`
2. 建 `ExpressionPlan`
3. GPT-SoVITS 支持 emotion voice profiles
4. Embedded channel 注册 `ExpressionAdapter`
5. Pipeline 调用 Expression Runtime
6. 插件注册 reducer/planner/adapter

我会稍微改成这个顺序：

### v0.1：Expression-only

先不做长期 Affect，只做本轮表达。

```text
dialogue_act + response_tone -> ExpressionPlan -> Adapters
```

目标是验证：

```text
TTS 风格切换
动作 allowlist
face/display 切换
adapter capability
orchestrator 调度
```

### v0.2：AffectState MVP

加入低维状态：

```text
valence
arousal
engagement
fatigue
```

先别加太多长期关系。

### v0.3：Appraisal + TTL events

加入：

```text
RawEvent -> AppraisalEvent -> Reducer
```

并支持：

```text
confidence
ttl
decay
dominant_causes
```

### v0.4：SocialState

加入：

```text
trust
familiarity
user_preference
interaction_style
```

注意这一步涉及长期记忆和隐私，要谨慎。

### v0.5：Plugin system

开放：

```text
AffectReducerPlugin
ExpressionPlannerPlugin
ExpressionAdapterPlugin
SafetyPolicyPlugin
```

这样更稳，因为你先把 runtime 行为跑通，再开放插件扩展。

---

## 20. 我会建议的最终接口

可以设计一个统一入口：

```ts
interface ExpressionRuntime {
  observe(event: RuntimeEvent): Promise<void>;

  getAffectSnapshot(scope?: AffectScope): Promise<AffectSnapshot>;

  planExpression(input: ExpressionInput): Promise<ExpressionPlan>;

  executeExpression(plan: ExpressionPlan): Promise<ExpressionExecutionResult>;
}
```

输入：

```ts
type ExpressionInput = {
  responseText: string;
  dialogueAct: DialogueAct;
  conversationContext: ConversationContext;
  userState?: UserStateSnapshot;
  deviceContext: DeviceContext;
  affectSnapshot?: AffectSnapshot;
  constraints?: ExpressionConstraints;
};
```

输出：

```ts
type ExpressionPlan = {
  id: string;
  intent: string;
  socialFunction: string;
  style: string;
  intensity: number;
  priority: "low" | "normal" | "high";
  interruptible: boolean;
  speech?: SpeechExpression;
  timeline: TimelineAction[];
  fallback: FallbackPolicy;
  safetyClass: SafetyClass;
};
```

执行结果：

```ts
type ExpressionExecutionResult = {
  status: "completed" | "partial" | "cancelled" | "blocked";
  executedActions: string[];
  skippedActions: {
    action: string;
    reason: string;
  }[];
  safetyModifications: {
    path: string;
    reason: string;
  }[];
};
```

---

## 21. 我最推荐的命名

你的命名已经比 emotion/action 好。我会进一步这样命名：

```text
Affective Runtime
Expression Runtime
Expression Adapter
```

如果是完整系统：

```text
Embodied Expression Runtime
```

内部模块：

```text
Appraisal Engine
Affect Store
Affect Reducer
Expression Intent Planner
Expression Realizer
Multimodal Orchestrator
Expression Safety Policy
Output Adapters
```

一个清晰的整体图：

```text
User / Sensors / System Events
        ↓
Appraisal Engine
        ↓
Affect Store + Reducers
        ↓
Affect Snapshot
        ↓
Expression Intent Planner
        ↓
Expression Realizer
        ↓
Expression Safety Policy
        ↓
Multimodal Orchestrator
        ↓
TTS / Motion / Face / Light / Haptics Adapters
```

---

## 22. 最核心的一条改进

你现在的核心句是：

> Affect 是内在状态，Expression 是外在表现，TTS/动作/屏幕只是输出通道。

我会扩展成：

> Appraisal 判断事件意义，Affect 维护持续状态，Regulation 决定可表现程度，Expression 生成本轮表达意图，Orchestrator 负责多模态同步，Adapters 只做设备实现。

最终一句话可以是：

**Affect 不直接驱动设备，Expression 不直接相信 LLM，Adapter 不决定语义；每一层只做自己的事。**

这是最稳的边界。






下面是我建议的工程计划：把它作为 server 的固定核心能力，名字可以定为 **Embodied Expression Runtime**。

**总体目标**
建立一套通用表达系统，让 Alia 能把“回复文本”扩展成“有状态、有语气、有动作、有安全约束的多模态表达”。

核心链路：

```text
Runtime Events
→ Appraisal Engine
→ Affect Store + Reducers
→ Regulation Policy
→ Expression Intent Planner
→ Expression Realizer
→ Expression Safety Policy
→ Multimodal Orchestrator
→ TTS / Motion / Face / Light / Haptics Adapters
```

**阶段计划**
1. **v0.1 Expression-only**
   先不做长期情感状态，只做本轮表达。
   - 新增 `ExpressionPlan`、`SpeechExpression`、`TimelineAction`、`ExpressionAdapterCapability`。
   - Pipeline 在最终回复文本产生后调用 Expression Runtime。
   - TTS 支持 `TtsRequest`，包含 `style`、`intensity`、`voice_profile`、`prosody`。
   - GPT-SoVITS 支持按 style 选择不同远端参考音频。
   - Embedded channel 先支持能力查询和抽象动作执行，比如 `small_nod`、`look_at_user`。

2. **v0.2 AffectState MVP**
   加入短期状态，但保持简单。
   - 状态字段先用 `valence`、`arousal`、`engagement`、`fatigue`。
   - 支持 baseline、instant、session 三层。
   - 每轮回复后保存 affect snapshot 和 dominant causes，方便 debug。
   - 不先做 `trust/familiarity`，避免过早碰长期关系建模。

3. **v0.3 Appraisal + TTL Events**
   加入事件解释层。
   - `RawEvent -> AppraisalEvent -> AffectReducer`。
   - Appraisal 支持规则 + minor LLM 分类。
   - 事件带 `confidence`、`scope`、`ttl`、`decay`、`evidence`。
   - Reducer 加 saturation、homeostasis、rate limit、conflict resolver。

4. **v0.4 Regulation + Social Function**
   让表达更稳。
   - 区分 `internal_affect` 和 `expressed_affect`。
   - 加 `social_function`：`acknowledge`、`comfort`、`explain`、`apologize`、`warn` 等。
   - Regulation 负责把负面/高激活状态转成合适的外显风格，比如 `calm_supportive`。

5. **v0.5 Plugin Extension**
   开放扩展点。
   - `AppraisalProvider`
   - `AffectReducer`
   - `ExpressionPlanner`
   - `ExpressionAdapter`
   - `ExpressionSafetyPolicy`
   - 所有插件注册都走统一 registry 和 dependency tree。

**推荐代码结构**

```text
packages/alia-server/src/expression/
  mod.rs
  types.rs
  runtime.rs
  appraisal.rs
  affect.rs
  regulation.rs
  planner.rs
  realizer.rs
  safety.rs
  orchestrator.rs
  adapters/
    mod.rs
    tts.rs
    channel.rs
```

TTS 侧建议把接口从纯文本升级为：

```rust
pub struct TtsRequest {
    pub text: String,
    pub style: Option<String>,
    pub intensity: f32,
    pub voice_profile: Option<String>,
    pub prosody: ProsodyControls,
}
```

动作侧不要让 LLM 直接控制设备。server 只生成抽象动作，embedded 自己决定怎么映射到 GPIO/电机。

**配置建议**

```toml
[policies.expression]
enabled = true
auto_motion_enabled = true
max_motion_intensity = 0.5
night_mode_minimal_expression = true

[expression.styles.gentle]
voice_profile = "alia_gentle"
rate = 0.92
energy = 0.38

[expression.styles.cheerful]
voice_profile = "alia_cheerful"
rate = 1.03
energy = 0.62
```

**数据库建议**

先建这几类表即可：

- `affect_events`：保存 appraisal/reducer 输入证据。
- `affect_snapshots`：保存每轮有效状态快照。
- `expression_plans`：保存本轮生成的表达计划。
- `expression_executions`：保存执行结果、降级、安全修改。
- `expression_capabilities`：记录通道/设备可用表达能力。

**最重要的工程边界**

Affect 不直接驱动设备。  
Expression 不直接相信 LLM。  
Adapter 不决定语义。  
Orchestrator 负责同步、取消、降级、安全限速。

我建议下一步先实现 **v0.1 Expression-only**，把 TTS 风格切换和 embedded 抽象动作链路跑通。长期情感状态等这条链路稳定后再接入。
