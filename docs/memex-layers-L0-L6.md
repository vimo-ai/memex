# Memex 分层架构 L0-L6（完整设计）

> 2026-06-06 汇编。三处交叉验证一致：历史会话设计讨论（2026-04~05）+ 2026-06-03 deep research 整理 + memex-rs 代码现状。
>
> 本文是 **L0-L6 的权威速查**，避免每次从会话历史里捞。压缩段（L0-L3）的实现细节见 [RFC-001-llm-compact](RFC-001-llm-compact.md)；竞品对比见 [competitive-analysis-2026-06](competitive-analysis-2026-06.md)。

---

## 核心范式

**原语 → 压缩 → 结晶，因果链可追溯。**

- **L0-L3 = 压缩段**：沿时间轴有损降维（液态对话逐层滤噪）。
- **L4-L6 = 结晶段**：从原语提取结构化知识（液态 → 固态）。
- 核心隐喻是**逐级结晶**，不是"记忆宫殿/大脑模型/知识图谱"等外部范式——那些在 2026 deep research 里被横向评估后否决（每个都是某隐喻硬套，该领域无成熟范式）。

## 六层总表

| 层 | 名称 | 定义 | 落地状态 |
|---|---|---|---|
| **L0** | 原文 | 不可变原语，永久保留（JSONL）。信息论最优：保留 L0，压缩是增值（Alberta ICLR 2026 证明 H(V0)>H(V1)>…） | ✅ 已实现 |
| **L1** | Observations | 工具调用级，独立于 L2 | ✅ 已实现 |
| **L2** | Talk Summary | 对话轮级，独立于 L1 | ✅ 已实现 |
| **L3** | Session Summary | 会话级，**依赖 L2**（原文直生会超 context；基于 L2 汇总 ~200tok×50=10k 可控） | ✅ 已实现 |
| **L4** | Knowledge Nodes | session 级提取 → 聚类（canonical_topic）→ **演化关系**（confirms / revises / supersedes / extends + evolution_narrative + final_confidence） | ✅ 已实现（keeta: 2867 nodes → 677 clusters + 685 singletons） |
| **L5** | Domain 域分类 | 项目级，cluster → domain，带 `scope: project-specific \| transferable` + 层级（parent_id），= "项目技术全景 / 活文档" | 🔶 部分实现(~22%)：`knowledge_clusters.l5_domain` 字段 + `idx_kc_l5` 索引已在，但 service 当前赋值 `None`；做过 demo（战役框架指导 L4 提取） |
| **L6** | 跨项目全局知识库 | 从各项目 transferable 的 L5 域聚合；`knowledge_domains` 表 `project_id = NULL` | ❌ schema 预留，暂缓 |

## 三条关键机制

### 1. 依赖/触发是 DAG，不是链

L1/L2 独立于原文，L3 依赖 L2，L4 从 session 提取——层之间是 DAG（一层可被多源触发，也可直读 L0）。两种写入入口都触发 DAG：

- `ingest`（细→粗）：事件进 L0 → 查谁订阅 L0 → 触发 processor → 递归
- `inject`（直接写某层）：粗粒度直接写指定层 → 触发该层下游 → 递归

processor **不区分上行/下行**，被触发就执行；粗→细展开和细→粗压缩用同一套机制。

### 2. L4↔L5 双向 bootstrap（核心，2026-04-26 确认）

- **自下而上**：L4 盲提（~6.8 nodes/session，无框架、不分主次）→ 聚类涌现出 domain。
- **自上而下**：L5 有源（README / 项目定义 / 战役框架 / 人工）时，反过来**指导 L4 提取**，从"盲提"变"有框架的提取"（知道往哪个域靠、置信度怎么给）。
- 两层**互相 bootstrap，不是固定先后流水线**；L5 是多源的（README / 自底聚合 / 沿引用图展开 / 人工 / 混合）。

> 对世界场景的同构映射：L5 自上而下指导 L4，等价于"lore / canon 自上而下约束八字事件结晶"（见虚拟人 `architecture-essence.md` §4「canon 重置八字涌现初始条件」）。

### 3. 检索

FTS + 向量 + RRF 融合（+ cross-encoder rerank），按层级 / 来源 / 上下文过滤。L0-L3 三层都建向量索引，用途不同：

- 快速路径：L3 → 定位相关 session → 深入 L1/L2 取细节
- 精准路径：直搜 L1/L2 具体操作/对话轮

## 两个代码库的定位

| | memex-rs | memex-core |
|---|---|---|
| 角色 | Claude 对话场景的具体实现 | 通用记忆引擎（**虚拟人在用**） |
| 落地 | L0-L4 已实现、L5 部分 | 声明式 DAG 驱动、EventSource 多源、LinkageStore 因果溯源 |

**关系**：memex-core 是 memex-rs 的**上位抽象**——memex-rs 的 L0-L4 是 memex-core DAG 引擎的一个具体实例化。两边共享认知模型，代码各走各的，迁移不急。

> 注意：L0-L6 这套是 memex-rs 的 **Coding/对话场景**层级。虚拟人世界场景的层级是 memex-core **场景自定义**注册的，要用 L4/L5/L6 需另做语义映射，且现阶段（Phase 2）不必落地。

## 待落地（最高优先级）

**记忆举证与综合置信度**（见 memory `project_evidence-confidence`）：当前 `knowledge_nodes.confidence` 是 LLM 自评分，需改为可举证的综合置信度：

```
置信度 = f(L0 溯源强度, 跨 session 印证次数, pipeline 版本权重, 时间衰减, supersede 状态, 访问频率)
```

竞品无人做好，这是独有设计空间。

## 出处索引

- `docs/RFC-001-llm-compact.md` — L0-L3 正式 RFC（compact 实现细节）
- `docs/competitive-analysis-2026-06.md` — 六层 + 竞品对比（deep research 产物）
- `memex-rs/src/compact/` — L0-L3 代码；`memex-rs/src/knowledge/` — L4-L5 代码（`store.rs` 含 `l5_domain` 字段 + `idx_kc_l5`）
- ETerm-memex Claude memory：`project_memex-architecture`（L0-L6 总表）、`project_evidence-confidence`（举证置信度）
- 设计讨论会话：`cbd2bb92` / `6f73445b`（L4↔L5 bootstrap）、`c37d47e8`（2026-06-03 整理）、`f10400bb` / `agent-a69a5eb13f3f9b685`（范式横评，否决记忆宫殿等）
