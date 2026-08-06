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

  /** 顶部导航标签（外壳独占）。 */
  nav: {
    overview: '总览',
    drilldown: '用量分析',
    detail: '明细',
    hosts: '主机',
    settings: '设置',
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

    /** 成本三态标签（actual / estimated / unavailable 永不折叠成一个数）。 */
    cost: {
      label: '成本',
      actual: '实际',
      estimated: '估算',
      partial: '部分缺失',
      unavailable: '成本不可用',
    },

    /** 覆盖三态（none 必须渲染为断裂，不是 0）。 */
    coverage: {
      full: '完整覆盖',
      partial: '部分覆盖',
      none: '无数据覆盖',
    },

    messageCount: '消息数',
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
      costDescription: '实际 / 估算 / 缺失分层，永不相加',
      costUnavailableLabel: '无可信成本',
      costUnavailableUnit: '条',
      costUnavailableHint: '这些记录不计入任何金额，也不当 0',
      volumeTitle: '消息与会话',
      volumeDescription: '已排除未完成消息',
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
      stored: '已存入钥匙串',
      absent: '未保存',
      requireHost: '请在下方主机列表点击「凭据」，选中一台主机后再保存',
      forHost: '当前主机',
      /** 明确告诉用户界面不会回显已保存的口令，避免误以为保存失败。 */
      neverEchoed: '已保存的口令不会回显，只显示是否存在。',
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

    /** 刷新间隔：下限有实测依据，见 description。 */
    refresh: {
      title: '刷新间隔',
      description: '实测一次全量扫描耗时 23 秒，因此本地间隔下限为 300 秒，避免相邻轮次互相重叠',
      local: '本地刷新间隔',
      remote: '远程刷新间隔',
      unitSeconds: '秒',
      minHint: '下限 300 秒',
      clamped: '低于下限，已自动调整为 300 秒',
      applyHint: '间隔在下次启动时应用到刷新调度器',
    },

    /** 手工价格覆盖表：经 IPC 走 prices.json 的原子写，前端不直接写文件。 */
    prices: {
      title: '价格覆盖',
      description: '手工维护的单价表，保存时经 IPC 原子替换 prices.json',
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
      empty: '暂无价格覆盖条目，估算成本会全部落到「成本不可用」',
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
      openUnavailable:
        '当前桌面壳未安装 opener/shell 插件，无法直接打开所在目录；请复制路径后在文件管理器中打开',
    },

    save: '保存设置',
    saved: '设置已保存',
    dirty: '有未保存的修改',
  },
} as const
