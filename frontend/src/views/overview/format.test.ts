import { describe, expect, it } from 'vitest'

import { formatCompact, formatCount, formatMoney } from './format'

/**
 * `formatCompact` 是承重件：它修的是一个真实视觉缺陷——11 位数的 token 计数在指标单元格
 * 宽度下与相邻指标重叠成一团不可读的字形。因此这里钉住的是**阈值边界与舍入**，而不是
 * 随手挑几个数。
 *
 * locale 在 `format.ts` 里被固定成 `en-US`，所以分组符与紧凑单位跨机器稳定。
 *
 * 与 Playwright 层的关系：下面「与 e2e 断言的字面量一致」一组，直接复用
 * `e2e/overview.spec.ts` 依赖的同一批输入输出对。改动 `formatCompact` 的任何边界，会先在
 * 这一层红掉；此时**必须同步修正 e2e 的期望值**，不允许把断言放松成"非空"。
 */
describe('overview/formatCompact 阈值边界', () => {
  it('千位以下原样输出，不加单位也不加分组符', () => {
    expect(formatCompact(0)).toBe('0')
    expect(formatCompact(1)).toBe('1')
    expect(formatCompact(999)).toBe('999')
  })

  it('K 在 1000 起跳', () => {
    expect(formatCompact(999)).toBe('999')
    expect(formatCompact(1_000)).toBe('1K')
  })

  it('M 在 999,950 起跳（1 位小数四舍五入的直接后果）', () => {
    expect(formatCompact(999_949)).toBe('999.9K')
    expect(formatCompact(999_950)).toBe('1M')
    expect(formatCompact(1_000_000)).toBe('1M')
  })

  it('B 与 T 的量级切换', () => {
    expect(formatCompact(999_999_999)).toBe('1B')
    expect(formatCompact(1_000_000_000)).toBe('1B')
    expect(formatCompact(999_999_999_999)).toBe('1T')
    expect(formatCompact(1_000_000_000_000)).toBe('1T')
  })

  it('T 是最大单位，更大的量级继续堆在 T 上', () => {
    expect(formatCompact(1_000_000_000_000_000)).toBe('1000T')
  })

  it('最多保留 1 位小数，按四舍五入', () => {
    expect(formatCompact(1_049)).toBe('1K')
    expect(formatCompact(1_050)).toBe('1.1K')
    expect(formatCompact(1_100)).toBe('1.1K')
    expect(formatCompact(9_999)).toBe('10K')
  })

  it('整数量级不带多余的 .0', () => {
    expect(formatCompact(2_000)).toBe('2K')
    expect(formatCompact(5_000_000)).toBe('5M')
  })

  it('负值保留符号', () => {
    expect(formatCompact(-1_500)).toBe('-1.5K')
  })
})

describe('overview/formatCompact 与 e2e 断言的字面量一致', () => {
  /**
   * 左列是 mock 数据集里的真实种子值，右列是 `e2e/overview.spec.ts` 正在断言的文本。
   * 两层必须逐字相同，否则单测绿而 e2e 红（或反之）。
   */
  const pairs: Array<[number, string]> = [
    [386_150, '386.2K'], // summary-token-input
    [29_550, '29.6K'], // summary-token-output
    [2_150, '2.2K'], // summary-token-reasoning
    [242_950, '243K'], // summary-token-cache = 231,200 + 11,750
    [1_136_161_924, '1.1B'], // 11 位数的真实归档量级，即当初重叠的那个场景
  ]

  for (const [input, expected] of pairs) {
    it(`${input} → ${expected}`, () => {
      expect(formatCompact(input)).toBe(expected)
    })
  }
})

describe('overview/formatCount', () => {
  it('加千分位分组，且不做任何量级压缩', () => {
    expect(formatCount(155_494)).toBe('155,494')
    expect(formatCount(386_150)).toBe('386,150')
    expect(formatCount(629_100)).toBe('629,100')
    expect(formatCount(231_200)).toBe('231,200')
    expect(formatCount(11_750)).toBe('11,750')
  })

  it('四位数起才有分组符', () => {
    expect(formatCount(0)).toBe('0')
    expect(formatCount(999)).toBe('999')
    expect(formatCount(1_000)).toBe('1,000')
  })

  it('11 位数保持完整精度——紧凑写法的 title 属性靠它兜住可读性', () => {
    expect(formatCount(1_136_161_924)).toBe('1,136,161,924')
  })

  it('负值保留符号', () => {
    expect(formatCount(-1_000)).toBe('-1,000')
  })
})

describe('overview/formatMoney', () => {
  it('固定 4 位小数并带美元符号（真实单条成本量级在 0.0004 附近）', () => {
    expect(formatMoney(0.0484)).toBe('$0.0484')
    expect(formatMoney(0.0075)).toBe('$0.0075')
    expect(formatMoney(0)).toBe('$0.0000')
  })

  it('不足 4 位补零，超出 4 位四舍五入', () => {
    expect(formatMoney(1)).toBe('$1.0000')
    expect(formatMoney(0.00004)).toBe('$0.0000')
    expect(formatMoney(0.00005)).toBe('$0.0001')
    expect(formatMoney(0.123456)).toBe('$0.1235')
  })

  it('大额金额仍加千分位', () => {
    expect(formatMoney(1234.5)).toBe('$1,234.5000')
  })
})
