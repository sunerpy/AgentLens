import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { ComponentProps } from 'react'

import type { ObservedModelPrice, PriceEntry } from '@/generated'
import { zh } from '@/i18n/zh'
import { shouldSuppressContextMenu } from '@/lib/contextMenuGuard'

import { PriceOverrideEditor } from './PriceOverrideEditor'

type Overrides = ComponentProps<typeof PriceOverrideEditor>['overrides']

const ANTHROPIC_PRICE: PriceEntry = {
  providerId: 'anthropic',
  modelId: 'claude-opus-4-8',
  inputPerMtok: 5,
  outputPerMtok: 25,
  cacheReadPerMtok: 0.5,
  cacheWritePerMtok: 6.25,
  extra: {},
}

const OPENAI_PRICE: PriceEntry = {
  providerId: 'openai',
  modelId: 'gpt-5.6-sol',
  inputPerMtok: 1.25,
  outputPerMtok: 10,
  cacheReadPerMtok: 0.125,
  cacheWritePerMtok: 1.25,
  extra: {},
}

const OBSERVED_MODELS: ObservedModelPrice[] = [
  {
    providerId: 'kiro-auth',
    modelId: 'claude-opus-4-8-high',
    usageCount: 25_958,
    matchKind: 'crossProvider',
    matchedPrice: ANTHROPIC_PRICE,
  },
  {
    providerId: 'kiro-auth',
    modelId: 'gpt-5.6-sol-xhigh',
    usageCount: 42_257,
    matchKind: 'crossProvider',
    matchedPrice: OPENAI_PRICE,
  },
  {
    providerId: 'myopenai',
    modelId: 'gpt-5.6-sol-high',
    usageCount: 12_000,
    matchKind: 'crossProvider',
    matchedPrice: OPENAI_PRICE,
  },
]

function overrides(): Overrides {
  return {
    rows: [
      {
        rowId: 'row-kiro',
        providerId: 'kiro-auth',
        modelId: 'claude-opus-4-8-high',
        inputPerMtok: '5',
        outputPerMtok: '25',
        cacheReadPerMtok: '0.5',
        cacheWritePerMtok: '6.25',
        extra: {},
      },
    ],
    catalog: {
      schemaVersion: 1,
      catalogVersion: 'test',
      updatedAt: '2026-08-08',
      currency: 'USD',
      entries: [ANTHROPIC_PRICE, OPENAI_PRICE],
      observedModels: OBSERVED_MODELS,
    },
    issues: [],
    dirty: false,
    saved: false,
    isPending: false,
    isSaving: false,
    error: null,
    refetch: vi.fn(),
    addRow: vi.fn(),
    addObservedModel: vi.fn(),
    deleteRow: vi.fn(),
    editRow: vi.fn(),
    submit: vi.fn(),
  }
}

afterEach(cleanup)

describe('PriceOverrideEditor observed model selectors', () => {
  it('keeps an unknown provider selectable and orders its observed models by usage', () => {
    const state = overrides()
    render(<PriceOverrideEditor overrides={state} />)

    const provider = screen.getByTestId('price-provider-0') as HTMLSelectElement
    const providerValues = Array.from(provider.options, (option) => option.value)
    expect(providerValues).toContain('kiro-auth')
    expect(providerValues).toContain('myopenai')
    expect(providerValues.indexOf('kiro-auth')).toBeLessThan(providerValues.indexOf('myopenai'))

    const model = screen.getByTestId('price-model-0') as HTMLSelectElement
    expect(model.disabled).toBe(false)
    expect(Array.from(model.options, (option) => option.value)).toEqual([
      '',
      'gpt-5.6-sol-xhigh',
      'claude-opus-4-8-high',
      '__custom__',
    ])
    expect(screen.getByTestId('price-observed-inferred-0').textContent).toContain(
      'gpt-5.6-sol-xhigh',
    )
    expect(screen.getByTestId('price-row-inferred-0')).toBeTruthy()

    fireEvent.change(model, { target: { value: 'gpt-5.6-sol-xhigh' } })
    expect(state.editRow).toHaveBeenCalledWith('row-kiro', {
      modelId: 'gpt-5.6-sol-xhigh',
      inputPerMtok: '1.25',
      outputPerMtok: '10',
      cacheReadPerMtok: '0.125',
      cacheWritePerMtok: '1.25',
    })
  })

  it('keeps unobserved override models selectable for existing and new rows', () => {
    const state = overrides()
    state.rows = [
      { ...state.rows[0], modelId: 'legacy-unobserved-model' },
      {
        ...state.rows[0],
        rowId: 'row-new',
        modelId: '',
      },
    ]
    render(<PriceOverrideEditor overrides={state} />)

    const existingModel = screen.getByTestId('price-model-0') as HTMLSelectElement
    expect(existingModel.value).toBe('legacy-unobserved-model')
    expect(Array.from(existingModel.options, (option) => option.value)).toContain(
      'legacy-unobserved-model',
    )
    expect(screen.queryByTestId('price-model-custom-0')).toBeNull()

    const newModel = screen.getByTestId('price-model-1') as HTMLSelectElement
    expect(Array.from(newModel.options, (option) => option.value)).toContain(
      'legacy-unobserved-model',
    )
  })
})

/** 每页 10 条，所以 26 个模型跨 3 页；三种状态各一批，用于筛选断言。 */
function manyObserved(): ObservedModelPrice[] {
  const models: ObservedModelPrice[] = []
  for (let index = 0; index < 12; index += 1) {
    models.push({
      providerId: 'kiro-auth',
      modelId: `Claude-Opus-Cross-${String(index).padStart(2, '0')}`,
      usageCount: 10_000 - index,
      matchKind: 'crossProvider',
      matchedPrice: ANTHROPIC_PRICE,
    })
  }
  for (let index = 0; index < 9; index += 1) {
    models.push({
      providerId: 'aws',
      modelId: `bedrock-normalized-${String(index).padStart(2, '0')}`,
      usageCount: 900 - index,
      matchKind: 'normalized',
      matchedPrice: OPENAI_PRICE,
    })
  }
  for (let index = 0; index < 5; index += 1) {
    models.push({
      providerId: 'private-provider',
      modelId: `private-model-${String(index).padStart(2, '0')}`,
      usageCount: 50 - index,
      matchKind: 'unknown',
      matchedPrice: null,
    })
  }
  return models
}

function renderManyObserved() {
  const state = overrides()
  state.catalog = { ...state.catalog!, observedModels: manyObserved() }
  render(<PriceOverrideEditor overrides={state} />)
  return state
}

function observedRowIds(): string[] {
  return Array.from(
    document.querySelectorAll<HTMLElement>('[data-testid^="price-observed-"]'),
  ).flatMap((element) => {
    const id = element.dataset.testid ?? ''
    return /^price-observed-(inferred|approximate|unknown)-\d+$/.test(id) ? [id] : []
  })
}

function pageText(): string {
  return screen.getByTestId('price-observed-page').textContent ?? ''
}

describe('PriceOverrideEditor 归档模型匹配的分页、筛选与搜索', () => {
  it('按每页 10 条分页，并展示匹配数与总数', () => {
    renderManyObserved()

    expect(observedRowIds()).toHaveLength(10)
    expect(screen.getByTestId('price-observed-total').textContent).toBe(
      zh.settings.prices.observedTotal(26, 26),
    )
    expect(pageText()).toBe(zh.settings.prices.observedPage(1, 3))
    expect((screen.getByTestId('price-observed-prev') as HTMLButtonElement).disabled).toBe(true)
    expect((screen.getByTestId('price-observed-next') as HTMLButtonElement).disabled).toBe(false)
  })

  it('翻到末页后按钮状态正确，且条目数是余下的那些', () => {
    renderManyObserved()

    fireEvent.click(screen.getByTestId('price-observed-next'))
    expect(pageText()).toBe(zh.settings.prices.observedPage(2, 3))
    expect(observedRowIds()).toHaveLength(10)

    fireEvent.click(screen.getByTestId('price-observed-next'))
    expect(pageText()).toBe(zh.settings.prices.observedPage(3, 3))
    expect(observedRowIds()).toHaveLength(6)
    expect((screen.getByTestId('price-observed-next') as HTMLButtonElement).disabled).toBe(true)

    fireEvent.click(screen.getByTestId('price-observed-prev'))
    expect(pageText()).toBe(zh.settings.prices.observedPage(2, 3))
  })

  it('按匹配状态筛选，只保留该状态的条目', () => {
    renderManyObserved()

    fireEvent.change(screen.getByTestId('price-observed-filter'), {
      target: { value: 'unknown' },
    })

    const ids = observedRowIds()
    expect(ids).toHaveLength(5)
    expect(ids.every((id) => id.startsWith('price-observed-unknown-'))).toBe(true)
    expect(screen.getByTestId('price-observed-total').textContent).toBe(
      zh.settings.prices.observedTotal(5, 26),
    )
    expect(pageText()).toBe(zh.settings.prices.observedPage(1, 1))
  })

  /** 分页最常见的缺陷：筛选后页码停在旧位置，用户看到一个空页并以为没有数据。 */
  it('筛选变化时页码重置回第一页', () => {
    renderManyObserved()

    fireEvent.click(screen.getByTestId('price-observed-next'))
    fireEvent.click(screen.getByTestId('price-observed-next'))
    expect(pageText()).toBe(zh.settings.prices.observedPage(3, 3))

    fireEvent.change(screen.getByTestId('price-observed-filter'), {
      target: { value: 'inferred' },
    })

    expect(pageText()).toBe(zh.settings.prices.observedPage(1, 2))
    expect(observedRowIds()).toHaveLength(10)
    expect(screen.queryByTestId('price-observed-empty')).toBeNull()
  })

  it('搜索变化时页码同样重置回第一页', () => {
    renderManyObserved()

    fireEvent.click(screen.getByTestId('price-observed-next'))
    expect(pageText()).toBe(zh.settings.prices.observedPage(2, 3))

    fireEvent.change(screen.getByTestId('price-observed-search'), {
      target: { value: 'private' },
    })

    expect(pageText()).toBe(zh.settings.prices.observedPage(1, 1))
    expect(observedRowIds()).toHaveLength(5)
  })

  it('搜索对 provider 与 model 都生效且忽略大小写', () => {
    renderManyObserved()

    // model 的真实大小写是 `Claude-Opus-Cross-00`，用全小写仍应命中。
    fireEvent.change(screen.getByTestId('price-observed-search'), {
      target: { value: 'claude-opus-cross-0' },
    })
    expect(observedRowIds()).toHaveLength(10)
    expect(screen.getByTestId('price-observed-total').textContent).toBe(
      zh.settings.prices.observedTotal(10, 26),
    )

    // provider 用大写片段命中小写的 `aws`。
    fireEvent.change(screen.getByTestId('price-observed-search'), {
      target: { value: 'AWS' },
    })
    expect(observedRowIds()).toHaveLength(9)
  })

  it('筛选与搜索可叠加，互斥条件给出空态而不是空白', () => {
    renderManyObserved()

    fireEvent.change(screen.getByTestId('price-observed-filter'), {
      target: { value: 'unknown' },
    })
    fireEvent.change(screen.getByTestId('price-observed-search'), {
      target: { value: 'private-model-0' },
    })
    expect(observedRowIds()).toHaveLength(5)

    // unknown 组里没有任何 kiro-auth 的模型，两个条件叠加后必然为空。
    fireEvent.change(screen.getByTestId('price-observed-search'), {
      target: { value: 'kiro' },
    })
    expect(observedRowIds()).toHaveLength(0)
    expect(screen.getByTestId('price-observed-empty').textContent).toContain(
      zh.settings.prices.observedNoMatch,
    )
    expect(screen.getByTestId('price-observed-total').textContent).toBe(
      zh.settings.prices.observedTotal(0, 26),
    )
  })

  it('清空搜索按钮恢复完整列表', () => {
    renderManyObserved()

    fireEvent.change(screen.getByTestId('price-observed-search'), {
      target: { value: 'private' },
    })
    expect(observedRowIds()).toHaveLength(5)

    fireEvent.click(screen.getByTestId('price-observed-clear-search'))

    expect((screen.getByTestId('price-observed-search') as HTMLInputElement).value).toBe('')
    expect(observedRowIds()).toHaveLength(10)
    expect(screen.queryByTestId('price-observed-clear-search')).toBeNull()
  })

  /** 筛选与搜索都不能让「补充覆盖价」失效：它是这个列表存在的理由。 */
  it('筛选并搜索之后，补充覆盖价仍然把那一条模型交给上层', () => {
    const state = renderManyObserved()

    fireEvent.change(screen.getByTestId('price-observed-filter'), {
      target: { value: 'unknown' },
    })
    fireEvent.change(screen.getByTestId('price-observed-search'), {
      target: { value: 'PRIVATE-MODEL-03' },
    })
    const ids = observedRowIds()
    expect(ids).toHaveLength(1)

    fireEvent.click(screen.getByTestId(ids[0].replace('price-observed-', 'price-observed-add-')))
    fireEvent.click(
      screen.getByTestId(`${ids[0].replace('price-observed-', 'price-observed-inline-')}-save`),
    )

    expect(state.addObservedModel).toHaveBeenCalledTimes(1)
    expect(vi.mocked(state.addObservedModel).mock.calls[0][0]).toMatchObject({
      providerId: 'private-provider',
      modelId: 'private-model-03',
      matchKind: 'unknown',
    })
  })

  it('归档里没有需要关注的模型时整块不渲染', () => {
    const state = overrides()
    state.catalog = { ...state.catalog!, observedModels: [] }
    render(<PriceOverrideEditor overrides={state} />)

    expect(screen.queryByTestId('price-observed')).toBeNull()
    expect(screen.queryByTestId('price-observed-filter')).toBeNull()
  })
})

/**
 * 「补充覆盖价」的就地展开。
 *
 * 原行为是直接往下方价格表追加一行，用户得自己滚下去找那一行；列表是分页的，那一行经常不在
 * 视口里。下面几条锁住三件容易回退的事：同时只展开一个、Esc 能收起、保存后收起。
 */
describe('PriceOverrideEditor 就地展开的覆盖价输入', () => {
  function inlineId(rowTestId: string): string {
    return rowTestId.replace('price-observed-', 'price-observed-inline-')
  }

  function expand(rowTestId: string) {
    fireEvent.click(screen.getByTestId(rowTestId.replace('price-observed-', 'price-observed-add-')))
  }

  /** unknown 组排在 inferred / approximate 之后，第一页看不到，所以先筛出来。 */
  function onlyUnknown() {
    fireEvent.change(screen.getByTestId('price-observed-filter'), {
      target: { value: 'unknown' },
    })
  }

  it('点击后在该行下方展开四个费率输入，并以候选价预填', () => {
    renderManyObserved()
    expect(screen.queryByTestId(inlineId('price-observed-inferred-0'))).toBeNull()

    expand('price-observed-inferred-0')

    const panel = screen.getByTestId(inlineId('price-observed-inferred-0'))
    expect(panel).toBeTruthy()
    expect(screen.getByTestId('price-observed-inferred-0').dataset.expanded).toBe('true')
    for (const [field, expected] of [
      ['inputPerMtok', '5'],
      ['outputPerMtok', '25'],
      ['cacheReadPerMtok', '0.5'],
      ['cacheWritePerMtok', '6.25'],
    ] as const) {
      expect(
        (
          screen.getByTestId(
            `${inlineId('price-observed-inferred-0')}-${field}`,
          ) as HTMLInputElement
        ).value,
      ).toBe(expected)
    }
  })

  /** 两个未保存草稿同时存在时，用户无法判断保存动作作用在哪一个上。 */
  it('同一时刻只展开一个，展开另一行会收起前一行', () => {
    renderManyObserved()

    expand('price-observed-inferred-0')
    expect(screen.getByTestId(inlineId('price-observed-inferred-0'))).toBeTruthy()

    expand('price-observed-inferred-1')

    expect(screen.queryByTestId(inlineId('price-observed-inferred-0'))).toBeNull()
    expect(screen.getByTestId(inlineId('price-observed-inferred-1'))).toBeTruthy()
    expect(screen.getByTestId('price-observed-inferred-0').dataset.expanded).toBe('false')
  })

  it('再点同一行的按钮就收起，按钮文案随之切换', () => {
    renderManyObserved()
    const button = () => screen.getByTestId('price-observed-add-inferred-0')

    expand('price-observed-inferred-0')
    expect(button().textContent).toBe(zh.settings.prices.inlineCollapse)

    fireEvent.click(button())

    expect(screen.queryByTestId(inlineId('price-observed-inferred-0'))).toBeNull()
    expect(button().textContent).toBe(zh.settings.prices.addObserved)
  })

  it('Esc 收起展开态，且不把草稿交给上层', () => {
    const state = renderManyObserved()
    expand('price-observed-inferred-0')

    fireEvent.keyDown(screen.getByTestId(inlineId('price-observed-inferred-0')), {
      key: 'Escape',
    })

    expect(screen.queryByTestId(inlineId('price-observed-inferred-0'))).toBeNull()
    expect(state.addObservedModel).not.toHaveBeenCalled()
  })

  it('取消按钮同样收起，且不提交', () => {
    const state = renderManyObserved()
    onlyUnknown()
    expand('price-observed-unknown-0')

    fireEvent.click(screen.getByTestId(`${inlineId('price-observed-unknown-0')}-cancel`))

    expect(screen.queryByTestId(inlineId('price-observed-unknown-0'))).toBeNull()
    expect(state.addObservedModel).not.toHaveBeenCalled()
  })

  it('保存把用户改过的费率一起交给上层，然后收起', () => {
    const state = renderManyObserved()
    onlyUnknown()
    expand('price-observed-unknown-0')
    const panel = inlineId('price-observed-unknown-0')

    fireEvent.change(screen.getByTestId(`${panel}-inputPerMtok`), { target: { value: '0.8' } })
    fireEvent.change(screen.getByTestId(`${panel}-outputPerMtok`), { target: { value: '3.2' } })
    fireEvent.click(screen.getByTestId(`${panel}-save`))

    expect(state.addObservedModel).toHaveBeenCalledTimes(1)
    const [model, rates] = vi.mocked(state.addObservedModel).mock.calls[0]
    expect(model).toMatchObject({ providerId: 'private-provider', modelId: 'private-model-00' })
    expect(rates).toMatchObject({
      inputPerMtok: '0.8',
      outputPerMtok: '3.2',
      cacheReadPerMtok: '0',
      cacheWritePerMtok: '0',
    })
    expect(screen.queryByTestId(panel)).toBeNull()
  })

  it('Enter 等价于保存', () => {
    const state = renderManyObserved()
    onlyUnknown()
    expand('price-observed-unknown-1')
    const panel = inlineId('price-observed-unknown-1')

    fireEvent.change(screen.getByTestId(`${panel}-inputPerMtok`), { target: { value: '1.5' } })
    fireEvent.keyDown(screen.getByTestId(panel), { key: 'Enter' })

    expect(state.addObservedModel).toHaveBeenCalledTimes(1)
    expect(vi.mocked(state.addObservedModel).mock.calls[0][1]).toMatchObject({
      inputPerMtok: '1.5',
    })
    expect(screen.queryByTestId(panel)).toBeNull()
  })

  /** 校验必须复用表格那一套，否则内联表单会放进一个表格拒收的值。 */
  it('非法费率禁用保存并复用既有的错误提示，Enter 也不提交', () => {
    const state = renderManyObserved()
    onlyUnknown()
    expand('price-observed-unknown-0')
    const panel = inlineId('price-observed-unknown-0')

    fireEvent.change(screen.getByTestId(`${panel}-inputPerMtok`), { target: { value: '-1' } })

    expect((screen.getByTestId(`${panel}-save`) as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByTestId('price-issue-number').textContent).toBe(
      zh.settings.prices.invalidNumber,
    )

    fireEvent.keyDown(screen.getByTestId(panel), { key: 'Enter' })
    expect(state.addObservedModel).not.toHaveBeenCalled()
    expect(screen.getByTestId(panel)).toBeTruthy()
  })

  /** 搜索框也是原生 input，同样保留粘贴用的原生右键菜单。 */
  it('填充源的搜索框是原生 input，右键菜单不受禁用影响', () => {
    renderManyObserved()
    onlyUnknown()
    expand('price-observed-unknown-0')
    const panel = inlineId('price-observed-unknown-0')

    fireEvent.click(screen.getByTestId(`${panel}-fill-toggle`))

    const search = screen.getByTestId(`${panel}-fill-search`)
    expect(search.tagName).toBe('INPUT')
    expect(shouldSuppressContextMenu(search)).toBe(false)
  })

  /** 展开态的输入必须是原生 input：contextMenuGuard 只对可编辑元素放行原生右键菜单。 */
  it('四个费率输入都是原生 input，粘贴用的原生右键菜单因此保留', () => {
    renderManyObserved()
    onlyUnknown()
    expand('price-observed-unknown-0')
    const panel = inlineId('price-observed-unknown-0')

    for (const field of [
      'inputPerMtok',
      'outputPerMtok',
      'cacheReadPerMtok',
      'cacheWritePerMtok',
    ]) {
      const element = screen.getByTestId(`${panel}-${field}`)
      expect(element.tagName).toBe('INPUT')
      expect(shouldSuppressContextMenu(element)).toBe(false)
    }
  })
})

/**
 * 「从已有定价填充」。
 *
 * 覆盖价原本要求逐个手敲四个费率，而权威单价就在内置目录里、用户自己定过的价就在覆盖价表里。
 * 下面几条锁住的是：填的是正确的四个数、填完仍可改且保存的是改后的值、填充不绕过费率校验、
 * 出处可见且可撤销，以及搜索框里的 Enter 不会把半成品行保存掉。
 */
describe('PriceOverrideEditor 从已有定价填充', () => {
  function inlineId(rowTestId: string): string {
    return rowTestId.replace('price-observed-', 'price-observed-inline-')
  }

  /** 展开 unknown-0（private-provider / private-model-00，无候选价，四个费率初值都是 0）。 */
  function openPicker(): string {
    fireEvent.change(screen.getByTestId('price-observed-filter'), { target: { value: 'unknown' } })
    fireEvent.click(screen.getByTestId('price-observed-add-unknown-0'))
    const panel = inlineId('price-observed-unknown-0')
    fireEvent.click(screen.getByTestId(`${panel}-fill-toggle`))
    return panel
  }

  function rate(panel: string, field: string): string {
    return (screen.getByTestId(`${panel}-${field}`) as HTMLInputElement).value
  }

  function fillOptionIds(): string[] {
    return Array.from(document.querySelectorAll<HTMLElement>('[data-testid*="-fill-option-"]')).map(
      (element) => element.dataset.testid ?? '',
    )
  }

  it('两类来源都列出，且默认收起填充源', () => {
    renderManyObserved()
    fireEvent.change(screen.getByTestId('price-observed-filter'), { target: { value: 'unknown' } })
    fireEvent.click(screen.getByTestId('price-observed-add-unknown-0'))
    const panel = inlineId('price-observed-unknown-0')

    expect(screen.queryByTestId(`${panel}-fill-panel`)).toBeNull()
    expect(screen.getByTestId(`${panel}-fill-toggle`).textContent).toBe(
      zh.settings.prices.fillTitle,
    )

    fireEvent.click(screen.getByTestId(`${panel}-fill-toggle`))

    expect(screen.getByTestId(`${panel}-fill-panel`)).toBeTruthy()
    expect(screen.getByTestId(`${panel}-fill-toggle`).textContent).toBe(
      zh.settings.prices.fillCollapse,
    )
    // 目录两条（anthropic / openai）+ 覆盖价一条（kiro-auth）。
    expect(fillOptionIds()).toEqual([
      `${panel}-fill-option-catalog-0`,
      `${panel}-fill-option-catalog-1`,
      `${panel}-fill-option-override-0`,
    ])
    expect(screen.getByTestId(`${panel}-fill-total`).textContent).toBe(
      zh.settings.prices.fillTotal(3, 3),
    )
  })

  it('选中一条目录定价把四个费率填进表单，并标出出处', () => {
    renderManyObserved()
    const panel = openPicker()
    expect(rate(panel, 'inputPerMtok')).toBe('0')

    // catalog-1 是 openai / gpt-5.6-sol：1.25 / 10 / 0.125 / 1.25。
    fireEvent.click(screen.getByTestId(`${panel}-fill-apply-catalog-1`))

    expect(rate(panel, 'inputPerMtok')).toBe('1.25')
    expect(rate(panel, 'outputPerMtok')).toBe('10')
    expect(rate(panel, 'cacheReadPerMtok')).toBe('0.125')
    expect(rate(panel, 'cacheWritePerMtok')).toBe('1.25')
    expect(screen.getByTestId(`${panel}-fill-origin`).textContent).toBe(
      zh.settings.prices.fillOrigin(zh.settings.prices.fillKindCatalog, 'openai', 'gpt-5.6-sol'),
    )
    expect(screen.queryByTestId(`${panel}-fill-adjusted`)).toBeNull()
  })

  it('已有覆盖价也能作为填充源，出处标为覆盖价', () => {
    renderManyObserved()
    const panel = openPicker()

    // override-0 是价格表里那一行：kiro-auth / claude-opus-4-8-high，5 / 25 / 0.5 / 6.25。
    fireEvent.click(screen.getByTestId(`${panel}-fill-apply-override-0`))

    expect(rate(panel, 'inputPerMtok')).toBe('5')
    expect(rate(panel, 'cacheWritePerMtok')).toBe('6.25')
    expect(screen.getByTestId(`${panel}-fill-origin`).textContent).toBe(
      zh.settings.prices.fillOrigin(
        zh.settings.prices.fillKindOverride,
        'kiro-auth',
        'claude-opus-4-8-high',
      ),
    )
  })

  /** 填充不等于保存：用户常常要在官方价基础上微调，保存必须写入调整后的值。 */
  it('填充后仍可编辑，保存写入的是编辑后的值而不是被填充的原值', () => {
    const state = renderManyObserved()
    const panel = openPicker()

    fireEvent.click(screen.getByTestId(`${panel}-fill-apply-catalog-1`))
    expect(state.addObservedModel).not.toHaveBeenCalled()

    fireEvent.change(screen.getByTestId(`${panel}-inputPerMtok`), { target: { value: '9' } })
    expect(screen.getByTestId(`${panel}-fill-adjusted`).textContent).toBe(
      zh.settings.prices.fillAdjusted,
    )

    fireEvent.click(screen.getByTestId(`${panel}-save`))

    expect(state.addObservedModel).toHaveBeenCalledTimes(1)
    const [model, rates] = vi.mocked(state.addObservedModel).mock.calls[0]
    expect(model).toMatchObject({ providerId: 'private-provider', modelId: 'private-model-00' })
    expect(rates).toMatchObject({
      inputPerMtok: '9',
      outputPerMtok: '10',
      cacheReadPerMtok: '0.125',
      cacheWritePerMtok: '1.25',
    })
  })

  it('撤销填充恢复填充前的草稿并撤掉出处标注', () => {
    renderManyObserved()
    // inferred-0 带候选价（5 / 25 / 0.5 / 6.25），撤销必须回到这组预填值。
    fireEvent.click(screen.getByTestId('price-observed-add-inferred-0'))
    const panel = inlineId('price-observed-inferred-0')
    fireEvent.click(screen.getByTestId(`${panel}-fill-toggle`))

    fireEvent.click(screen.getByTestId(`${panel}-fill-apply-catalog-1`))
    expect(rate(panel, 'inputPerMtok')).toBe('1.25')

    fireEvent.click(screen.getByTestId(`${panel}-fill-undo`))

    expect(rate(panel, 'inputPerMtok')).toBe('5')
    expect(rate(panel, 'outputPerMtok')).toBe('25')
    expect(rate(panel, 'cacheReadPerMtok')).toBe('0.5')
    expect(rate(panel, 'cacheWritePerMtok')).toBe('6.25')
    expect(screen.queryByTestId(`${panel}-fill-origin`)).toBeNull()
    expect(screen.queryByTestId(`${panel}-fill-undo`)).toBeNull()
  })

  it('搜索按 provider 与 model 过滤填充源且忽略大小写', () => {
    renderManyObserved()
    const panel = openPicker()

    fireEvent.change(screen.getByTestId(`${panel}-fill-search`), { target: { value: 'OPENAI' } })
    expect(fillOptionIds()).toEqual([`${panel}-fill-option-catalog-1`])
    expect(screen.getByTestId(`${panel}-fill-total`).textContent).toBe(
      zh.settings.prices.fillTotal(1, 3),
    )

    fireEvent.change(screen.getByTestId(`${panel}-fill-search`), {
      target: { value: 'CLAUDE-OPUS-4-8-HIGH' },
    })
    expect(fillOptionIds()).toEqual([`${panel}-fill-option-override-0`])

    fireEvent.click(screen.getByTestId(`${panel}-fill-clear-search`))
    expect(fillOptionIds()).toHaveLength(3)
  })

  it('来源筛选只保留该类，互斥条件给出空态', () => {
    renderManyObserved()
    const panel = openPicker()

    fireEvent.change(screen.getByTestId(`${panel}-fill-kind`), { target: { value: 'override' } })
    expect(fillOptionIds()).toEqual([`${panel}-fill-option-override-0`])

    fireEvent.change(screen.getByTestId(`${panel}-fill-search`), { target: { value: 'openai' } })
    expect(fillOptionIds()).toHaveLength(0)
    expect(screen.getByTestId(`${panel}-fill-empty`).textContent).toContain(
      zh.settings.prices.fillNoMatch,
    )
  })

  it('超过五条时分页，筛选变化把页码重置回第一页', () => {
    const state = overrides()
    state.catalog = {
      ...state.catalog!,
      observedModels: manyObserved(),
      entries: Array.from({ length: 12 }, (_, index) => ({
        ...OPENAI_PRICE,
        modelId: `catalog-model-${String(index).padStart(2, '0')}`,
        inputPerMtok: index,
      })),
    }
    render(<PriceOverrideEditor overrides={state} />)
    const panel = openPicker()

    expect(fillOptionIds()).toHaveLength(5)
    expect(screen.getByTestId(`${panel}-fill-page`).textContent).toBe(
      zh.settings.prices.fillPage(1, 3),
    )
    expect((screen.getByTestId(`${panel}-fill-prev`) as HTMLButtonElement).disabled).toBe(true)

    fireEvent.click(screen.getByTestId(`${panel}-fill-next`))
    expect(screen.getByTestId(`${panel}-fill-page`).textContent).toBe(
      zh.settings.prices.fillPage(2, 3),
    )
    expect(fillOptionIds()).toEqual([
      `${panel}-fill-option-catalog-5`,
      `${panel}-fill-option-catalog-6`,
      `${panel}-fill-option-catalog-7`,
      `${panel}-fill-option-catalog-8`,
      `${panel}-fill-option-catalog-9`,
    ])

    fireEvent.change(screen.getByTestId(`${panel}-fill-kind`), { target: { value: 'override' } })
    expect(screen.getByTestId(`${panel}-fill-page`).textContent).toBe(
      zh.settings.prices.fillPage(1, 1),
    )
  })

  /** 填充只是把数字塞进输入框，它必须仍然经过表格那一套校验。 */
  it('填进一个非法费率同样禁用保存并给出既有提示', () => {
    const state = overrides()
    state.rows = [{ ...state.rows[0], inputPerMtok: '-1' }]
    state.catalog = { ...state.catalog!, observedModels: manyObserved() }
    render(<PriceOverrideEditor overrides={state} />)
    const panel = openPicker()

    fireEvent.click(screen.getByTestId(`${panel}-fill-apply-override-0`))

    expect(rate(panel, 'inputPerMtok')).toBe('-1')
    expect((screen.getByTestId(`${panel}-save`) as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByTestId('price-issue-number').textContent).toBe(
      zh.settings.prices.invalidNumber,
    )

    fireEvent.keyDown(screen.getByTestId(panel), { key: 'Enter' })
    expect(state.addObservedModel).not.toHaveBeenCalled()
  })

  /** 搜索框里的 Enter 若冒泡到表单就会把半成品行保存掉；Esc 只该收起填充源。 */
  it('填充面板吞掉 Enter，Esc 只收起填充源而不收起整个表单', () => {
    const state = renderManyObserved()
    const panel = openPicker()

    fireEvent.keyDown(screen.getByTestId(`${panel}-fill-panel`), { key: 'Enter' })
    expect(state.addObservedModel).not.toHaveBeenCalled()
    expect(screen.getByTestId(`${panel}-fill-panel`)).toBeTruthy()

    fireEvent.keyDown(screen.getByTestId(`${panel}-fill-panel`), { key: 'Escape' })
    expect(screen.queryByTestId(`${panel}-fill-panel`)).toBeNull()
    expect(screen.getByTestId(panel)).toBeTruthy()
  })
})
