/**
 * 中文文案字典（单一字典模块，刻意不引入 i18n 框架）。
 *
 * 结构约定（**并行开发的关键**，由 W8 预置任务确定）：
 * - `nav` / `common` 由外壳任务拥有，视图任务不要改；
 * - 每个视图各有一个顶层 section（`overview` / `drilldown` / `detail` / `hosts` / `settings`），
 *   由对应 todo **独占**，只在自己的块内增删键，从而五个并行 worker 不会互相覆盖；
 * - 组件内不写裸中文串，一律经此字典引用（`node scripts/check-i18n.mjs` 会强制）。
 */
export const zh = {
  appName: 'AgentLens',
  tagline: 'AI 编码工具用量统计',

  /** 自绘窗口标题栏（外壳独占）。 */
  titlebar: {
    minimize: '最小化',
    maximize: '最大化',
    restore: '向下还原',
    close: '关闭',
  },

  /** 左侧栏导航标签（外壳独占）。 */
  nav: {
    overview: '总览',
    drilldown: '用量分析',
    detail: '明细',
    hosts: '主机',
    settings: '设置',
    diagnostics: '日志',
  },

  /**
   * 左侧栏自身的操作文案（外壳独占）。
   *
   * 收缩态只剩图标，读屏用户拿不到可见文字，所以每个导航项在该状态下都要用 `nav` 里的
   * 同一条文案兜 `aria-label` 与 `title`；这里只放侧栏控件自己的文案。
   */
  sidebar: {
    label: '主导航',
    collapse: '收起侧栏',
    expand: '展开侧栏',
    hide: '隐藏侧栏',
    show: '显示侧栏',
    pin: '固定侧栏（挤压内容）',
    unpin: '浮动侧栏（覆盖内容）',
    resize: '调整侧栏宽度',
  },

  /**
   * 颜色主题（外壳独占）。键名与 `src/app/theme/themes.ts` 的 `THEME_KEYS` 必须一一对应，
   * 新增主题时三处同改：`index.css` 的 token 块、`themes.ts` 的注册表、这里的名称。
   */
  theme: {
    label: '颜色主题',
    names: {
      light: '石墨浅色',
      dark: '石墨深色',
      forest: '苔原绿',
      ocean: '深海蓝',
      amber: '暖砂',
      violet: '夜紫',
    },
    modes: {
      light: '浅色 · 中性',
      dark: '深色 · 中性',
      forest: '浅色 · 冷绿',
      ocean: '深色 · 蓝青',
      amber: '浅色 · 暖陶',
      violet: '深色 · 紫',
    },
  },

  /** 跨视图共用文案（外壳独占；需要新增共用词时请与外壳约定，避免与视图 section 重复）。 */
  common: {
    loading: '加载中…',
    empty: '该区间无记录',
    errorTitle: '加载失败',
    errorCode: '错误码',
    retry: '重试',
    invokeFailed: '调用失败',
    unknownError: '未知错误',

    /** 区间预设（半开区间 [start, end)）。 */
    range: {
      today: '今天',
      last7Days: '7 天',
      last30Days: '30 天',
      /** 日历对齐的本季度 / 本年，到今天为止；不是「回看 92 天」那种滚动窗口。 */
      thisQuarter: '本季度',
      thisYear: '本年',
      custom: '自定义',
      timezone: '报表时区',
      weekStart: '周起日',
    },

    /** 粒度。 */
    granularity: {
      hour: '小时',
      day: '天',
      week: '周',
      month: '月',
    },

    /** token 分类标签（五个原子桶 + 唯一派生量，绝不合并展示）。 */
    tokens: {
      label: 'Token',
      input: '输入',
      output: '输出',
      reasoning: '推理',
      cacheRead: '缓存读取',
      cacheWrite: '缓存写入',
      totalInput: '总输入',
    },

    /**
     * 成本三态标签（actual / estimated / unavailable 永不折叠成一个数）。
     *
     * **这三个词是按「谁算的」命名的，不是按「准不准」命名的。** 上一版把 `actual` 叫「实际」，
     * 那是错的：`CostSource::Actual` 的来源是 OpenCode 消息记录里自带的 `cost` 字段
     * （`crates/agentlens-core/src/source/opencode.rs`），那个数是 OpenCode 用它自己的定价表
     * 算出来的，**不是云厂商账单**。它与我们的估算本质同类，区别只是谁算的：上游算的 vs 本地算的。
     * 叫「实际」会让用户把它读成「你真的花了这么多」，于是反复追问两个金额为什么差几千倍——
     * 真正的答案是「只有 117 条记录的上游附带了自己的估算值」。
     *
     * Rust 侧枚举名保持 `CostSource::Actual` 不改：那个判别值以字符串 `"actual"` 落在归档库里
     * （`query.rs` 的 `"actual" => Ok(CostSource::Actual)`），改名要么破坏已归档的每一行，要么
     * 让 Rust 标识符与线上取值分叉，两者都比现状更糟；而 `Actual` 在代码里指「来源自带的值」
     * 这个语义本来就清楚。误导只发生在用户看到的中文上，所以只改这里。
     */
    cost: {
      label: '成本',
      actual: '来源自带',
      estimated: '本地估算',
      partial: '部分缺失',
      unavailable: '目录无价',
    },

    /** 覆盖三态（none 必须渲染为断裂，不是 0）。 */
    coverage: {
      full: '完整覆盖',
      partial: '部分覆盖',
      none: '无数据覆盖',
    },

    /**
     * 三个计数互不替代：`messageCount` 只数 `granularity = 'message'` 的记录（一条 = 一条消息），
     * `sessionRecordCount` 只数 `granularity = 'session'` 的记录（一条 = 一整个会话的汇总），
     * `activeSessionCount` 是跨粒度的 `count(DISTINCT session_id)`。token 与成本跨粒度求和，
     * 所以「消息数」偏小并不意味着金额偏小。第二个键刻意不叫「会话数」——那会与第三个混淆。
     */
    messageCount: '消息数',
    sessionRecordCount: '会话汇总记录数',
    activeSessionCount: '活跃会话数',
  },

  // ↓↓↓ todo 15 独占：只在本块内增删键 ↓↓↓
  overview: {
    title: '总览',
    subtitle: '按报表时区聚合的用量、成本与覆盖状况',

    range: {
      label: '统计区间',
      /** 半开区间 [start, end)：结束日本身不计入统计。 */
      halfOpenHint: '结束日不计入',
      pickEndHint: '先选开始日，再选结束日',
      prevMonth: '上一月',
      nextMonth: '下一月',
      apply: '应用',
      clear: '重选',
      weekdays: ['一', '二', '三', '四', '五', '六', '日'],
      /**
       * 自定义区间：起始与截止各一个独立日期选择器。用户填写的截止日是**含当天**的，
       * 派发给后端前才 +1 天转成半开区间的 endDateExclusive。
       */
      customTitle: '自定义区间',
      startDate: '起始日期',
      endDate: '截止日期',
      endDateInclusiveHint: '截止日期含当天，本区间统计到该日 23:59',
      invalidOrder: '截止日期不能早于起始日期',
      requireBothDates: '请同时填写起始与截止日期',
    },

    granularity: {
      label: '粒度',
      auto: '自动',
      autoHint: '随区间跨度自适应',
      pinnedHint: '已固定，预设切换不再改动',
    },

    summary: {
      tokenTitle: 'Token 用量',
      tokenDescription: '四类分列，五个原子桶不折叠',
      /** 展示用分组：缓存 = 缓存读取 + 缓存写入；查询层刻意不预合并。 */
      tokenCache: '缓存',
      tokenTotalHint: '总输入 = 输入 + 缓存读取 + 缓存写入',
      costTitle: '成本',
      /**
       * 上一版这里写的是「实际 / 估算 / 缺失分层，永不相加」。「永不相加」是写给实现者的
       * 内部约束（约束本身不变，见 `costTiers.ts`），用户不需要知道我们内部怎么防串味；
       * 他需要知道眼前这个数是谁算的、算的是哪一批记录。
       */
      costDescription: '按本机价目表估算；上游自带的金额与目录里没有价格的记录分开列',
      costCoverage: (records: string, tokens: string) =>
        `${records} 条记录 · ${tokens} 可计费 Token`,

      /**
       * 用户第三次问「实际 $83.5228 和估算 $312,235.4418 为什么差那么多」。前两轮分别补了
       * 覆盖量标注、改成竖排分层加单价对比，都没救回来——因为两轮都在回答「怎么解释这两个
       * 数字的差异」，而真正的缺陷有两条：
       *
       * 1. **其中一个数字的名字是错的。** 详见 `common.cost` 的说明：`actual` 是上游自带的
       *    估算值，不是账单。叫「实际」时，任何差异解释都会被读成「实际值才对，估算错了」。
       * 2. **它不该占主视觉。** 实测 289,834 条记录里只有 117 条（0.03%）带上游金额，
       *    99.97% 走本地估算。把覆盖 0.03% 的数与覆盖 99.97% 的并排等重摆放，就是在邀请相减。
       *
       * 所以这一版：本地估算是唯一的主数字；来源自带降为默认折叠的次要披露，非零时才出现
       * （零覆盖时连折叠入口都没有，那个 $0 从此不会出现在任何视觉层级上）。
       */
      costPrimaryHint: '本机价目表 × 可计费 Token 算出的估算值，不是账单',
      costTierShare: (share: string) => `覆盖 ${share}`,
      costTierShareUnknown: '本区间没有可计费 Token',
      costUnitPriceLabel: '单价（每百万可计费 Token）',
      /** 该层没有可计费 Token 时单价无定义；写「—」而不是 $0，$0 会被读成「不要钱」。 */
      costUnitPriceUndefined: '—',
      costUnitPriceHint: '单价已除掉覆盖量差异，是这张卡里唯一可以横向比较的数',
      costSourceShow: (records: string) => `另有 ${records} 条记录自带上游金额`,
      costSourceHide: '收起上游自带金额',
      costSourceExplain:
        '这个金额来自 OpenCode 记录里自带的 cost 字段，由 OpenCode 用它自己的价目表算出，同样不是云厂商账单。',
      costSourceIncomparable: (actualRecords: string, estimatedRecords: string) =>
        `它只覆盖 ${actualRecords} 条记录，本地估算覆盖 ${estimatedRecords} 条，两者是两批完全不同的记录：金额既不能相加也不能相减，要比就比单价。`,
      costEstimatedNoCoverage:
        '本区间没有记录走本地估算，上面的 $0 是「没有记录」，不是「估算出来是 0」。',
      costNoCoverage: '本区间没有任何记录带成本，$0 是「没有数据」，不是「没有花钱」。',
      costUnavailableLabel: '目录无价',
      costUnavailableUnit: '条',
      /**
       * 上一版写的是「这些记录不计入任何金额，也不当 0」——那是实现视角，描述的是我们的
       * 数据结构而不是用户的处境。用户要知道的是「为什么这些用量没金额」。
       */
      costUnavailableHint: '这些模型在价目表里查不到价格，所以它们的用量算不出金额',
      /**
       * 「部分缺失」原来只报一个数，用户看不出**什么**没有价格；补上清单后又出现第二个问题：
       * 表头的「本范围内 21,947 条」与清单合计 50,923 条对不上，用户以为算错了。
       *
       * 根因是两个口径并排：表头的 `unavailableCount` 按当前范围与筛选统计记录数，而清单当时
       * 来自 `price_catalog_get`，那条 SQL 对整张 usage_record 分组、不带时间条件，是全库口径。
       * 上一轮加的声明性文案救不回来——两个不同口径的数字并排出现，用户只会读成同一个。
       *
       * 现在清单改由趋势查询的 model 分组派生，与表头**同范围、同筛选、同 is_incomplete 排除**，
       * 因此清单每条相加恰好等于表头总数：它是那个总数的分解，不是第二个统计量。全库清单降级为
       * 拿不到分组时的兜底，放在独立分区并单独标注口径，绝不与范围内的记录数并排。
       */
      missingShow: '哪些没有价格？',
      missingHide: '收起缺价明细',
      missingSummary: (models: number, records: string) =>
        `本范围内有 ${models} 个模型没有价格，合计 ${records} 条记录因此没有成本`,
      missingRecordUnit: '条记录',
      missingExpand: (hidden: number) => `展开其余 ${hidden} 个`,
      missingCollapse: '只看用量最高的几个',
      missingNoIdentity: '这些记录在归档中已不存在对应模型，通常是价格改动后的历史残留',
      /** 明示「清单是表头总数的分解」，让用户可以直接相加核对而不是怀疑算错。 */
      missingRangeScopeHint: '模型与记录数都按当前时间范围与筛选统计，逐条相加等于上面的总数',
      /** 残差只在为正时出现；为 0 是常态，出现即说明分解不完整，必须让用户看见。 */
      missingUnattributed: (records: string) => `另有 ${records} 条记录未能归到具体模型`,
      /**
       * 三类成因（变体后缀未覆盖 / 目录确实缺条目 / provider 隔离）在现有 DTO 里区分不了：
       * 三者都表现为 `matchKind: 'unknown'` 且 `matchedPrice: null`。按模型名猜等于在前端硬编码
       * 一份会过期的名单，所以这里只说明「分不了」并指向设置页，不编造分类。
       */
      missingCauseHint:
        '本页不区分成因：缺价的三种情况（模型名带运行档位后缀、目录里确实没有这个模型、provider 之间价格本就不同）在当前数据里表现相同',
      missingFixHint: '要补价格：设置 → 价格覆盖 → 归档中的模型匹配，筛选「价格未知」',
      /** 兜底分区：只有拿不到范围内分组时才出现，标题与口径说明必须自带范围限定词。 */
      missingArchiveTitle: '全库口径的缺价模型',
      missingArchiveScopeHint:
        '当前范围的分组数据不可用，这份清单按整个归档统计，与上面的范围内记录数不是同一口径，不能相加',
      volumeTitle: '消息与会话',
      volumeDescription: '已排除未完成消息',

      /**
       * 会话级记录只有启用了会话级计量采集源的用户才有，绝大多数用户恒为 0。因此这段说明
       * 与卡内那格数字都只在非零时出现：常态下不放一张干瘪的「0」，非零时则必须在页面上
       * **直接可读**（不藏进 tooltip）——否则用户会把「金额有值、消息数没算进去」当成缺陷。
       */
      granularityNoteTitle: '两种粒度并存',
      granularityNote: (messageCount: string, sessionRecordCount: string) =>
        `本区间含 ${messageCount} 条消息级记录与 ${sessionRecordCount} 条会话汇总记录。会话汇总记录来自只提供会话级计量的采集源，一条记录代表一整个会话的汇总，因此不计入「消息数」；上方的 Token 与成本已经把两种粒度都算进去了。`,
    },

    trend: {
      title: '用量趋势',
      metricLabel: '指标',
      metricTokens: 'Token 合计',
      metricTokensHint: '五个原子桶之和，不读取源库的 tokens.total',
      metricCost: '成本',
      metricCostHint: '实际与估算分两条曲线，不相加',
      seriesTokens: 'Token 合计',
      seriesActual: '实际成本',
      seriesEstimated: '估算成本',
      legendGap: '无覆盖（断裂）',
      legendPartial: '部分覆盖',
      empty: '该区间没有任何时间桶',
      allGap: '该区间全部时间桶都没有数据覆盖',
      tooltipNoCoverage: '该时间桶没有归档覆盖，因此不画点（不是 0）',
      tooltipZeroUsage: '覆盖完整但无用量，真实为 0',

      /**
       * 分组维度。`tool`（工具）对应归档里的 source 字段：opencode / codex 这些是采集来源，
       * 用户口语里叫「工具」，因此界面用「工具」而内部标识符仍是 source。
       */
      groupLabel: '分组维度',
      groupNone: '不分组',
      groupModel: '按模型',
      groupAgent: '按 agent',
      groupTool: '按工具',
      groupNoneHint: '合计一条曲线，成本按实际 / 估算分列',
      groupModelHint: '按 (provider, model) 分列；同一模型的 variant 合并',
      groupAgentHint: '按 agent_key 分列，与用量分析二级一致',
      groupToolHint: '按采集来源分列（opencode / codex 等）',
      groupCostSeriesHint: '分组视图只画实际成本曲线；估算与缺失见悬浮提示',
      groupOther: '其他',
      groupOtherHint: (kept: number, total: number) =>
        `曲线过多会读不出趋势，因此只画 token 合计最高的 ${kept} 项，其余 ${total - kept} 项合并为「其他」`,
      groupTopHint: (total: number) => `该区间共 ${total} 项，全部单独成线`,
      groupEmpty: '该区间没有可分组的维度值',
      groupLoading: '正在按分组维度取数…',
      groupOtherNote: '「其他」= 合计 − 已列出各项，负值按 0 处理（区间边界上可能出现舍入差）',

      /**
       * 图例单选。分组线一多，用户想确认某一条的走势就得在六七条交叠的曲线里用眼睛描 ——
       * 所以图例项本身就是筛选器：点一次只留这一条，再点一次回到全览。
       *
       * 覆盖带在单选态下**保持显示**：覆盖是窗口的属性，取自未分组的总量序列，与选了哪个分组
       * 无关（见 `trendGrouping.ts` 顶部说明）。单选时把斜纹带一起藏掉会让用户以为
       * 这条线所在的区间是完整采集的。
       */
      legendSelectHint: '点击图例只看那一条，再点一次回到全部',
      legendSelectedHint: (label: string) => `当前只显示「${label}」，覆盖带仍按整体统计`,
      legendShowAll: '显示全部曲线',
      legendItemTitle: (label: string) => `只看「${label}」`,

      /**
       * 「部分覆盖」的成因。原先 UI 只给结论：用户看到斜纹带却不知道是哪台机器、哪个源缺数据，
       * 只能猜。后端的 Partial 判定是「并非每个 (主机, 源) 对都完整覆盖该桶」，所以只要启用了
       * 多个采集源、其中任一在那个桶没有采集区间，整桶就是部分覆盖 —— 这里把那些对逐个列出来。
       *
       * 两种成因分开表述：`partial: true` 是「采集区间只压住了桶的一部分」（多半是那段时间里
       * 才开始采集，或采集中断过），`partial: false` 是「这个桶里完全没有它的采集区间」。
       */
      coverageReasonTitle: '为什么不是完整覆盖',
      coverageReasonIntro: '下列「主机 / 源」在该时间桶没有完整的采集区间：',
      coverageReasonPartial: '只覆盖了一部分',
      coverageReasonMissing: '完全没有采集区间',
      coverageReasonPair: (hostId: string, source: string) => `${hostId} / ${source}`,
      coverageReasonUnknown: '归档里没有留下能解释该时间桶的采集区间记录',
      coverageReasonHint:
        '覆盖按 (主机, 源) 逐对判定：任一对缺区间，整桶即为部分覆盖；仍在进行中的时间桶不在此列',

      /**
       * 尚未结束的时间桶。采集区间是 `[since, now]`，而 Full 要求区间完整压住整个桶 ——
       * 所以「当前时刻所在的那个桶」永远不可能完整覆盖，跟采集健不健康无关。把它的 (主机, 源)
       * 当缺口列出来，等于每天每次刷新都刷屏，还会把真正漏采的历史桶埋掉。
       *
       * 因此：斜纹带与「部分覆盖」徽章保留（数据确实不全，不该看起来和历史桶一样可信），
       * 但理由换成「还没结束」，且不进缺口清单。
       *
       * `coverageInProgressMissingIntro` 是例外口子：某个 (主机, 源) 在这个桶里**完全**没有
       * 采集区间，说明它这一整段都没采到过，这跟「桶还没结束」无关，仍要报。
       */
      coverageInProgressTitle: '这个时间桶还没结束',
      coverageInProgressNote:
        '采集区间最多只能到当前时刻，所以还不是完整覆盖 —— 这是预期结果，不是漏采',
      coverageInProgressMissingIntro:
        '下列「主机 / 源」在该时间桶完全没有采集区间，这与「还没结束」无关：',
      coverageInProgressTag: '进行中',
    },
  },

  // ↓↓↓ todo 16 独占：只在本块内增删键 ↓↓↓
  drilldown: {
    title: '用量分析',
    subtitle: '来源 → agent → 模型 三级展开；区间与时区与总览共享同一状态',

    /** 只读区间标签：区间选择器归总览（todo 15）所有，本视图只消费共享状态。 */
    rangeLabel: '区间',
    timezoneLabel: '时区',
    hostFilter: '主机过滤',
    hostAll: '全部主机',
    hostUnavailable: '主机列表不可用',

    levelSourceStep: '一级',
    levelSourceTitle: '来源（source）',
    levelSourceHint: 'source 是开放字符串；新增采集来源无需改动本视图',
    levelAgentStep: '二级',
    levelAgentTitle: 'agent',
    levelAgentHint: '按 agent_key 聚合，展示最近一次出现的 agent 名称',
    levelModelStep: '三级',
    levelModelTitle: '模型（provider × model）',
    levelModelHint: '按 (provider_id, model_id) 聚合；variant 默认折叠，可展开逐项查看',

    columnSource: '来源',
    columnAgent: 'agent',
    columnModel: '模型',
    columnShare: '占比',
    /** 仅在本区间真有会话级记录时随该列一起出现；否则每级表格都会多出一整列 0。 */
    sessionRecordNote:
      '会话汇总记录数一条代表一整个会话的汇总，不计入「消息数」——所以一行可能消息数为 0 而 Token 与成本有值。',
    shareNote: '占比 = 本行 token 合计 ÷ 本级 token 合计（输入 + 输出 + 推理 + 缓存读 + 缓存写）',

    summaryTitle: '当前筛选合计',
    tokenTotalLabel: 'Token 合计',
    breadcrumbLabel: '分析路径',
    variantNone: '默认（无 variant）',
    variantsLabel: 'variant',
    sourcesLabel: '来源',
    agentsLabel: 'agent',
    modelsLabel: '模型',
    rowsLabel: '归档行',
    expandVariants: '展开 variant',
    collapseVariants: '收起 variant',
    drillHint: '点击行名进入下一级',
    unavailableBadge: '成本缺失',

    /** 空区间失败场景要求的逐字文案，故本块自持一份，不复用 common.empty。 */
    empty: '该区间无记录',
    emptyHint: '换一个区间或主机过滤条件再试',
  },

  // ↓↓↓ todo 17 独占：只在本块内增删键 ↓↓↓
  detail: {
    title: '明细',
    description: '逐条消息记录，服务端分页；未完成消息在此可见，但不参与图表聚合。',
    tableLabel: '消息明细表',

    /** 列头。时间列带报表时区后缀，缓存列是两个原子桶之和。 */
    columns: {
      time: '时间',
      host: '主机',
      agent: 'Agent',
      model: '模型',
      cache: '缓存',
      flags: '标记',
    },

    /** 缓存列的悬浮说明前缀，后接 common.tokens.cacheRead / cacheWrite 的实际数值。 */
    cacheHint: '缓存读取 + 缓存写入',

    /** is_incomplete 徽标：进行中或被中断的消息，无完成时间、token 通常为 0。 */
    incomplete: '未完成',
    incompleteHint: '消息进行中或已中断，无完成时间；图表聚合会排除该行。',

    filters: {
      legend: '过滤条件',
      host: '主机',
      agent: 'Agent',
      model: '模型',
      incomplete: '完成状态',
      any: '全部',
      incompleteOnly: '仅未完成',
      completeOnly: '仅已完成',
      reset: '清空过滤',
    },

    /** 分页。行数一律取自服务端 total_count，不用当前页长度。 */
    pager: {
      previous: '上一页',
      next: '下一页',
      totalRows: '共',
      rowsUnit: '行',
      showing: '当前显示第',
      to: '至',
      pageSize: '每页',
    },

    /** 过滤后无行时的空态（total_count = 0）。 */
    emptyFiltered: '当前过滤条件下无记录',
  },

  // ↓↓↓ todo 18 独占：只在本块内增删键 ↓↓↓
  hosts: {
    title: '主机管理',
    subtitle: '本机自动注册；远端主机经 SSH 采集，口令只存系统钥匙串',

    local: {
      title: '本机',
      badge: '自动注册',
      registering: '正在注册本机…',
      registered: '已注册',
      unregistered: '尚未注册',
      register: '注册本机',
      identityLabel: '机器标识',
      hostIdLabel: '主机 ID',
      identityUnavailable: '无法确定本机身份',
      /** machine-id 缺失时后端错误自带 remediation，这里只做前缀。 */
      identityHint: '本机卡需要稳定的机器标识才能避免同一台机器被重复统计。',
      defaultDisplayName: '本机',
    },

    add: {
      title: '添加 SSH 主机',
      description: '填写 ssh 别名或主机名；用户名可留空以沿用 ~/.ssh/config',
      displayName: '显示名称',
      displayNamePlaceholder: '例如：构建机',
      host: 'ssh 别名 / 主机',
      hostPlaceholder: '例如：build-box 或 build-box.internal',
      user: '用户名（可选）',
      userPlaceholder: '例如：ci',
      identityFile: '密钥路径（可选）',
      identityFilePlaceholder: '例如：~/.ssh/id_ed25519',
      dataDir: '远端数据目录覆盖（可选）',
      dataDirPlaceholder: '留空则按 XDG_DATA_HOME 自动发现',
      machineIdHash: '远端机器标识',
      machineIdHashHint: '由「测试连接」自动读取，无需填写',
      /** 自动填入后输入框转只读：值来自远端，手改会把这台机器的用量记到别人名下。 */
      machineIdHashFilled: '已从远端读取，不可手改；改动上方主机或用户名后需重新测试连接',
      test: '测试连接',
      testing: '正在连接…',
      testingHint: '正在执行远端基础探测；完整连接测试最多运行 20 秒，期间可随时取消。',
      cancelTest: '取消测试',
      cancelling: '正在取消…',
      submit: '添加主机',
      submitting: '正在添加…',
      requireHost: '请先填写 ssh 别名或主机名',
      requireDisplayName: '请填写显示名称',
      requireMachineIdHash: '请先点「测试连接」读取远端机器标识',
      /** Windows 上 ssh-agent 不可用时的引导；无法在 Linux 上实测，故仅渲染文案。 */
      agentUnavailable:
        'Windows 上若 ssh-agent 不可用，请在上方「密钥路径」中直接选择私钥文件（例如 %USERPROFILE%\\.ssh\\id_ed25519），并把 passphrase 存入下方钥匙串。',
    },

    probe: {
      successTitle: '连接成功',
      failureTitle: '连接失败',
      architecture: '远端架构',
      dataDir: '数据目录',
      xdgDataHome: 'XDG_DATA_HOME',
      xdgUnset: '未设置',
      availableSpace: '可用空间',
      machineIdSource: 'machine-id 来源',
      remediationLabel: '处理建议',
    },

    credentials: {
      title: '钥匙串凭据',
      description:
        '口令与密钥 passphrase 只写入系统钥匙串（Windows 凭据管理器 / Linux libsecret），绝不落配置文件',
      password: '登录口令',
      passphrase: '密钥 passphrase',
      placeholder: '输入后保存；保存完成即从界面清除',
      save: '保存到钥匙串',
      saving: '正在保存…',
      remove: '删除',
      stored: '已存入，采集时自动使用',
      absent: '未保存',
      requireHost: '请在下方主机列表点击「凭据」，选中一台主机后再保存',
      forHost: '当前主机',
      /** 明确告诉用户界面不会回显已保存的口令，避免误以为保存失败。 */
      neverEchoed: '已保存的口令不会回显，只显示是否存在。',
      /**
       * 「什么时候会用到」这条因果链。凭据已真正接进 ssh 认证链路，所以可以承诺自动使用；
       * 但仅对远端主机有意义 —— 本机走本地文件读取，压根不经过 SSH。
       */
      whenUsed:
        '适用场景：远端主机需要用口令登录（或私钥带 passphrase）时，把口令存在这里，采集时会自动完成认证，无需每轮手动输入。本机直接读本地文件，不涉及 SSH，因此不需要凭据。',
      /**
       * `CredentialStatus.present` = 已存入钥匙串 **且** 随包 askpass 助手存在。助手缺失时
       * 口令无法送达 ssh，后端因此报 present=false，界面不能谎称「已配好」。
       */
      absentHint: '「未保存」也包含这种情况：口令已存入，但随包口令助手缺失，无法送达 ssh。',
    },

    list: {
      title: '主机列表',
      empty: '还没有任何主机',
      emptyHint: '先注册本机，或用上方表单添加一台 SSH 主机',
      columnHost: '主机',
      columnKind: '类型',
      columnState: '状态',
      columnLastSuccess: '最近成功',
      columnActions: '操作',
      kindLocal: '本机',
      kindSsh: 'SSH',
      manageCredentials: '凭据',
      refresh: '刷新',
      refreshing: '正在刷新…',
      alreadyRunning: '已在刷新中',
      started: '已开始刷新',
      /**
       * 调度器键是 (host_id, source)，所以一台主机的每个启用采集源各有一份独立状态：
       * OpenCode 空闲而 Claude Code 出错是正常且必须分别显示的情形。
       */
      columnSources: '采集源',
      sourcesUnavailable: '无启用的采集源',
      /**
       * 采集源勾选：`hosts.enabled_sources` 的默认值只有 `'opencode'`，另外三个适配器必须
       * 在这里显式勾上，否则本机的 ~/.codex、~/.claude、~/.hermes 根本不会被扫描 ——
       * 「总览里只有 opencode」就是这么来的。可勾选项来自 hosts_supported_sources，
       * 不在前端写死。
       */
      sourcesEditTitle: '启用采集源',
      sourcesEditHint: '勾选要采集的源并保存；未勾选的源不会被扫描，也不会出现在总览里',
      sourcesFirstScanHint: '首次启用一个新源时，那一轮要全量扫描它的数据目录，会比平时慢',
      sourcesSave: '保存采集源',
      sourcesSaving: '正在保存…',
      sourcesSaved: '采集源已保存',
      /** 后端对空集合会报「采集源配置无效」，但先在前端说清后果比让它往返一趟更好。 */
      sourcesRequireOne: '至少要启用一个采集源，否则这台主机不会被采集',
      /**
       * 新启用一个源后立刻替它跑一轮，并把这一轮的状态说出来。
       *
       * 后端本来就会自动跑：本机 Auto 槽新建时 `last_completed_utc=None`，
       * `next_due_utc` 因此是 `None`，语义是「下一个 tick 立即到期」，而 tick 是每秒一次。
       * 但那一轮对用户完全不可见 —— 刷新状态只在 invalidate 时重取，所以一次几秒到几分钟的
       * 全量扫描期间，卡片可能一直停在「状态未知」，看起来就像勾选没生效。显式触发一轮把
       * 那 0~1 秒的空窗去掉，也让既有的进度通道立刻把该源推成「采集中」。
       */
      sourcesScanning: '正在为新启用的采集源做首次采集…',
      sourcesScanStarted: '首次采集已开始，完成后总览即会包含这个源',
      sourcesScanAlreadyRunning: '这台主机已有采集在进行，新源会在同一轮里被采到',
      /** 刷新全部主机的全部启用采集源；每个 (主机, 采集源) 各起一轮。 */
      refreshAll: '全部刷新',
      refreshingAll: '正在刷新全部…',
      refreshAllHint: '对全部主机的每个启用采集源各触发一轮采集',
      refreshAllDone: (rounds: number) => `已触发 ${rounds} 轮采集`,
      refreshAllNoHosts: '没有可刷新的主机',
      delete: '删除',
      deleting: '正在删除…',
      never: '从未成功',
      interrupted: '上轮被中断',
      remediationLabel: '处理建议',
      /** 采集轮次失败，与「页面加载失败」不是一回事，故不复用 common.errorTitle。 */
      errorTitle: '采集失败',
      statusUnavailable: '状态未知',
      stateIdle: '空闲',
      stateRunning: '刷新中',
      stateError: '出错',
      triggerAuto: '定时',
      triggerManual: '手动',
    },
  },

  // ↓↓↓ todo 19 独占：只在本块内增删键 ↓↓↓
  settings: {
    title: '设置',
    subtitle: '所有设置持久化到归档库的 app_settings 表，没有第二处存储',

    /** 外观：主题名称本身在 `zh.theme`（外壳独占），此处只放本卡片的说明文案。 */
    appearance: {
      title: '外观',
      description: '切换颜色主题；选择即时生效，无需点保存',
      persistHint: '主题与其他设置一样存在 app_settings 表，重启后仍生效',
    },

    /** 报表时区与周起日：桶边界由 Rust 查询层按这两个值计算。 */
    report: {
      title: '报表设置',
      description: '时区与周起日决定所有视图的时间桶边界',
      timezone: '报表时区',
      timezoneHint: '只能从 IANA 时区列表中选择，不接受自由输入',
      timezoneEffect: '时区变更后趋势图的桶边界随之改变',
      weekStart: '周起日',
      weekStartHint: '影响周粒度桶的起始日，默认周一',
      weekStartMonday: '周一',
      weekStartSunday: '周日',
    },

    /**
     * 自动刷新与间隔。下限 600 秒由后端 `MIN_AUTO_REFRESH_INTERVAL_MS` **拒绝**而不是钳制，
     * 所以界面也报错而不静默改写：用户填了 60 秒却被悄悄改成 600，他会一直以为在每分钟采集。
     */
    refresh: {
      title: '自动刷新',
      description:
        '一次远端轮次要起六个 ssh/scp 进程，实测全量扫描 23 秒，因此间隔下限为 600 秒（10 分钟），避免相邻轮次互相重叠',
      autoRefresh: '定时自动刷新',
      autoRefreshOn: '已开启',
      autoRefreshOff: '已关闭',
      autoRefreshHint: '关闭后所有采集源转为手动，仅在你点刷新时采集',
      local: '本地刷新间隔',
      remote: '远程刷新间隔',
      unitSeconds: '秒',
      minHint: '下限 600 秒',
      /** 后端会拒绝，所以这里必须是错误而非「已自动调整」。 */
      belowFloor: '低于下限 600 秒，后端会拒绝保存，请填 600 或更大的值',
      malformed: '请填写正整数秒数',
      applyHint: '间隔在保存后应用到刷新调度器',
    },

    /** 手工价格覆盖表：经 IPC 走 prices.json 的原子写，前端不直接写文件。 */
    prices: {
      title: '价格覆盖',
      description: '从内置官方目录选择并按需覆盖；保存时经 IPC 原子替换 prices.json',
      catalogVersion: '内置目录版本',
      catalogUpdated: '价格核对日期',
      catalogOffline: '目录随应用提供，不会在启动或查询时联网抓取',
      chooseProvider: '请选择 provider',
      chooseModel: '请选择 model',
      customEntry: '手动输入…',
      customProvider: '手动 provider',
      customModel: '手动 model',
      providerAmazonBedrock: 'Amazon Bedrock',
      providerAnthropic: 'Anthropic',
      providerGoogle: 'Google',
      providerOpenAi: 'OpenAI',
      observedTitle: '归档中的模型匹配',
      observedDescription:
        '近似匹配会使用同 provider 候选价；推断价来自未知网关下的跨 provider 模型匹配；未知模型保持成本未知',
      /**
       * 观测模型条数随归档增长没有上限（单个网关下就可能十几个模型），所以列表分页并配
       * 状态筛选与子串搜索。筛选项与三个 matchKind 分组一一对应，`observedFilterAll`
       * 是不筛选。
       */
      observedFilterLabel: '匹配状态',
      observedFilterAll: '全部',
      observedSearchLabel: '搜索 provider / model',
      observedSearchPlaceholder: '输入片段即可，忽略大小写',
      observedTotal: (matched: number, total: number) => `${matched} / ${total} 条`,
      observedPage: (page: number, pages: number) => `第 ${page} / ${pages} 页`,
      observedPrevPage: '上一页',
      observedNextPage: '下一页',
      /** 与 `empty` 不同：那是「没有覆盖价」，这是「有观测数据但当前条件筛不出来」。 */
      observedNoMatch: '没有符合当前筛选与搜索条件的模型；放宽条件或清空搜索框',
      observedClearSearch: '清空搜索',
      approximate: '近似匹配',
      inferred: '推断价',
      inferredHint: '当前 provider 不在价格目录中，按模型身份采用直连 provider 的候选价格',
      unknown: '价格未知',
      matchedTo: '候选价格',
      addObserved: '补充覆盖价',
      /**
       * 就地展开的覆盖价输入：点「补充覆盖价」不再直接往下方价格表追加一行，而是在被点的那一行
       * 下面展开四个费率输入框。原行为要求用户自己滚到价格表去找刚追加的那一行，而列表是分页的，
       * 那一行经常根本不在视口里。
       */
      inlineSave: '保存这一条',
      inlineCancel: '取消',
      inlineCollapse: '收起',
      inlineKeyboardHint: 'Enter 保存，Esc 收起',
      /**
       * 从已有定价填充：内联表单不再要求逐个手敲四个费率，可以直接挑一条已有定价套进去。
       * 两类来源都保留 —— 内置目录是官方单价，已有覆盖价是用户自己为同类模型定过的价，
       * 后者对「给伪模型/未知网关定价」这条路径尤其有用。填充只写进输入框，不代替保存，
       * 因为用户常常要在官方价基础上再微调。
       */
      fillTitle: '从已有定价填充',
      fillCollapse: '收起填充源',
      fillHint: '选中一条后四个费率会填进上面的输入框，仍可继续修改，保存写入的是修改后的值',
      fillKindLabel: '定价来源',
      fillKindAll: '全部',
      fillKindCatalog: '内置目录',
      fillKindOverride: '已有覆盖价',
      fillSearchLabel: '搜索 provider / model',
      fillSearchPlaceholder: '输入片段即可，忽略大小写',
      fillClearSearch: '清空搜索',
      fillTotal: (matched: number, total: number) => `${matched} / ${total} 条`,
      fillPage: (page: number, pages: number) => `第 ${page} / ${pages} 页`,
      fillPrevPage: '上一页',
      fillNextPage: '下一页',
      fillNoMatch: '没有符合条件的定价；放宽来源筛选或清空搜索框',
      fillEmpty: '没有可用于填充的定价：内置目录为空，也还没有已保存的覆盖价',
      fillApply: '使用这条',
      fillRateSummary: (input: string, output: string, cacheRead: string, cacheWrite: string) =>
        `输入 ${input} · 输出 ${output} · 缓存读 ${cacheRead} · 缓存写 ${cacheWrite}`,
      /** 填充后保留出处，用户回头核对时能看出这四个数字的依据。 */
      fillOrigin: (kind: string, providerId: string, modelId: string) =>
        `已填充自${kind}：${providerId} / ${modelId}`,
      fillAdjusted: '已在填充值基础上手动调整',
      fillUndo: '撤销填充',
      usageCount: '条记录',
      modelWithUsage: (modelId: string, usageCount: number) => `${modelId} · ${usageCount} 条`,
      variantHint: 'variant 不参与定价：同一模型的所有 variant 命中同一条目',
      reasoningHint: '推理 token 不单独计价，缓存读写各自按自己的单价计算',
      columnProvider: 'provider',
      columnModel: 'model',
      columnInput: '输入 / Mtok',
      columnOutput: '输出 / Mtok',
      columnCacheRead: '缓存读取 / Mtok',
      columnCacheWrite: '缓存写入 / Mtok',
      columnActions: '操作',
      addRow: '新增条目',
      deleteRow: '删除',
      save: '保存价格',
      saved: '价格已保存',
      empty: '暂无手工价格覆盖；成本将优先使用内置目录，未匹配模型仍保持未知',
      invalidBlank: 'provider 与 model 不能为空',
      invalidNumber: '单价必须是非负的有限数',
      invalidDuplicate: '同一 provider 与 model 只能有一条记录',
    },

    /** 归档库位置：由桌面壳在启动时写入 app_settings 的只读键。 */
    archive: {
      title: '归档库位置',
      description: 'SQLite 归档库的绝对路径，由桌面壳在启动时写入',
      unavailable: '归档库位置暂不可用',
      copy: '复制路径',
      copied: '已复制',
      open: '打开所在目录',
      openUnsupported: '当前环境不是桌面壳，无法调用文件管理器；请复制路径后手动打开',
      /** 与 openUnsupported 的区别：壳存在但系统拒绝了，Linux 上多为缺文件管理器的 D-Bus 服务。 */
      openFailed: '系统未能打开文件管理器；请复制路径后手动打开',
    },

    save: '保存设置',
    saved: '设置已保存',
    dirty: '有未保存的修改',
  },

  /**
   * 日志与反馈视图。
   *
   * 反馈文案刻意不提供「把日志贴进 issue」的按钮：日志正文可能带主机名、SSH 目标与绝对
   * 路径，自动脱敏是黑名单，漏一条就永久公开在公共 issue 里。预填只带构建与平台常量，
   * 日志片段由用户自己复制他看过的内容。
   */
  diagnostics: {
    title: '日志与反馈',
    subtitle: '桌面壳的运行日志，以及带环境信息的问题反馈入口',

    logs: {
      title: '运行日志',
      description: '按时间倒序，最新在最上；桌面壳没有控制台，出错信息只在这里',
      refresh: '刷新',
      levelLabel: '级别',
      levelAll: '全部',
      copy: '复制当前列表',
      copied: '已复制',
      copyFailed: '复制失败；请手动选中日志文本后复制',
      openDirectory: '打开日志目录',
      directoryLabel: '日志目录',
      retention: '单文件上限 2 MiB，最多保留 3 个文件（合计不超过 6 MiB），超出后最旧的自动删除',
      empty: '暂无日志记录；桌面壳一旦记录内容就会出现在这里',
      emptyFiltered: '当前级别没有记录',
      count: '条记录',
      /**
       * 日志时间原本按**运行机器**的本地偏移写入（Rust `chrono::Local`），与其他视图的报表
       * 时区无关，同一轮采集在这里读 09:58、在主机页读 01:58。现已统一按报表时区渲染，因此
       * 必须把口径写在界面上——不写的话用户无从判断这个时刻属于哪个时区。
       */
      timezoneLabel: (timezone: string) => `时间（${timezone}）`,
      timezoneHint: '日志时间与其他视图一致，按设置里的报表时区渲染；原始记录带写入时的本地偏移',
      /** 与 openUnsupported 的区别见 zh.settings.archive。 */
      openUnsupported: '当前环境不是桌面壳，无法调用文件管理器；请复制目录路径后手动打开',
      openFailed: '系统未能打开文件管理器；请复制目录路径后手动打开',
      envHint: '需要更详细的日志时，设置 RUST_LOG=debug 后重启应用',
    },

    feedback: {
      title: '问题反馈',
      description: '在 GitHub 上新建 issue，预填应用版本与平台信息',
      open: '去 GitHub 提交反馈',
      openUnsupported: '当前环境不是桌面壳，无法调用浏览器；请复制链接后手动打开',
      openFailed: '系统未能打开浏览器；请复制链接后手动打开',
      copyLink: '复制反馈链接',
      copied: '已复制',
      environmentTitle: '将随反馈一起提交的环境信息',
      privacyNotice:
        '预填内容只有下面这几项，不含主机地址、用户名、机器标识哈希、归档路径或任何凭据。日志片段请自行复制你确认过的内容后粘贴。',
      appVersion: '应用版本',
      os: '操作系统',
      arch: '架构',
      webview: 'WebView 版本',
      webviewUnknown: '未知',
    },
  },
} as const
