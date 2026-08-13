import { describe, expect, it } from 'vitest'

import {
  DEFAULT_LOCAL_INTERVAL_SECONDS,
  DEFAULT_REMOTE_INTERVAL_SECONDS,
  MIN_INTERVAL_SECONDS,
  SETTING_KEY_ARCHIVE_PATH,
  SETTING_KEY_AUTO_REFRESH_ENABLED,
  SETTING_KEY_AUTO_UPDATE_ENABLED,
  SETTING_KEY_LOCAL_INTERVAL_MS,
  SETTING_KEY_REMOTE_INTERVAL_MS,
  SETTING_KEY_UPDATE_PROXY_URL,
  autoRefreshEnabledFromSettings,
  autoUpdateEnabledFromSettings,
  intervalSecondsFromSettings,
  parseIntervalSeconds,
  updateProxyIssue,
} from './settingsKeys'

/**
 * 这些常量与键名是 Rust 侧的**镜像**，不是前端自己定的口味：
 * `MIN_INTERVAL_SECONDS` 对应 `MIN_AUTO_REFRESH_INTERVAL_MS = 600_000`，
 * 三个键名对应 `src-tauri/src/tray.rs` 里的 `SETTING_KEY_*`。拼错一个字符就是设置写进了
 * 后端永远不会读的键，界面看着保存成功、调度器毫无变化。所以这里逐字钉住。
 *
 * 下限语义也一并钉住：后端**拒绝**低于 600 秒的写入而不是钳制，因此
 * `parseIntervalSeconds` 必须返回 issue 而不是悄悄改成 600。
 */
describe('settingsKeys/键名逐字对齐 Rust', () => {
  it('三个刷新键与归档路径键的拼写是唯一事实源', () => {
    expect(SETTING_KEY_LOCAL_INTERVAL_MS).toBe('refresh.localIntervalMs')
    expect(SETTING_KEY_REMOTE_INTERVAL_MS).toBe('refresh.remoteIntervalMs')
    expect(SETTING_KEY_AUTO_REFRESH_ENABLED).toBe('refresh.autoRefreshEnabled')
    expect(SETTING_KEY_AUTO_UPDATE_ENABLED).toBe('update.autoInstallEnabled')
    expect(SETTING_KEY_UPDATE_PROXY_URL).toBe('update.proxyUrl')
    expect(SETTING_KEY_ARCHIVE_PATH).toBe('archive.path')
  })

  it('下限 600 秒 = 后端的 600000 毫秒，缺省值不低于下限', () => {
    expect(MIN_INTERVAL_SECONDS).toBe(600)
    expect(MIN_INTERVAL_SECONDS * 1000).toBe(600_000)
    expect(DEFAULT_LOCAL_INTERVAL_SECONDS).toBeGreaterThanOrEqual(MIN_INTERVAL_SECONDS)
    expect(DEFAULT_REMOTE_INTERVAL_SECONDS).toBeGreaterThanOrEqual(MIN_INTERVAL_SECONDS)
  })
})

describe('settingsKeys/updateProxyIssue', () => {
  it('留空表示沿用系统代理，三种支持协议与 URL 内认证均可保存', () => {
    for (const raw of [
      '',
      '   ',
      'http://127.0.0.1:7890',
      'https://proxy.example:8443',
      'socks5://127.0.0.1:1080',
      'http://user:pass@proxy.example:8080',
    ]) {
      expect(updateProxyIssue(raw)).toBeNull()
    }
  })

  it('缺协议、缺主机与无效文本在保存前归为 malformed', () => {
    for (const raw of ['proxy.example:8080', 'http://', 'not a proxy']) {
      expect(updateProxyIssue(raw)).toBe('malformed')
    }
  })

  it('拒绝未支持协议和代理端点之外的 URL 组成', () => {
    expect(updateProxyIssue('ftp://proxy.example:21')).toBe('unsupportedScheme')
    expect(updateProxyIssue('http://proxy.example:8080/path')).toBe('unsupportedShape')
    expect(updateProxyIssue('http://proxy.example:8080?mode=fast')).toBe('unsupportedShape')
    expect(updateProxyIssue('http://proxy.example:8080#fragment')).toBe('unsupportedShape')
  })
})

describe('settingsKeys/parseIntervalSeconds 下限校验', () => {
  it('恰好等于下限被接受（边界含端点，与后端 `< 600000` 才拒绝一致）', () => {
    expect(parseIntervalSeconds('600')).toEqual({ seconds: 600, issue: null })
  })

  it('低于下限一秒即拒绝，且不返回可保存的秒数', () => {
    expect(parseIntervalSeconds('599')).toEqual({ seconds: null, issue: 'belowFloor' })
  })

  it('常见的过小值全部归入 belowFloor 而不是被静默改写', () => {
    for (const raw of ['1', '60', '300', '599']) {
      expect(parseIntervalSeconds(raw)).toEqual({ seconds: null, issue: 'belowFloor' })
    }
  })

  it('下限之上原样采用', () => {
    expect(parseIntervalSeconds('601')).toEqual({ seconds: 601, issue: null })
    expect(parseIntervalSeconds('900')).toEqual({ seconds: 900, issue: null })
    expect(parseIntervalSeconds('86400')).toEqual({ seconds: 86_400, issue: null })
  })

  it('首尾空白容忍', () => {
    expect(parseIntervalSeconds('  900  ')).toEqual({ seconds: 900, issue: null })
  })

  it('空串、零、负数、小数、非数字一律 malformed', () => {
    for (const raw of ['', '   ', '0', '-1', '-600', '600.5', '6e2', 'abc', '十分钟', '600s']) {
      expect(parseIntervalSeconds(raw)).toEqual({ seconds: null, issue: 'malformed' })
    }
  })

  it('两类 issue 都不给出 seconds，调用方无法误持久化被拒的值', () => {
    for (const raw of ['', '0', '-1', '59', '599', 'abc']) {
      expect(parseIntervalSeconds(raw).seconds).toBeNull()
    }
  })
})

describe('settingsKeys/autoRefreshEnabledFromSettings', () => {
  it('键缺失视为开启——早于该开关的安装不能因此停止采集', () => {
    expect(autoRefreshEnabledFromSettings({})).toBe(true)
  })

  it('只有 false / 0 / off / no 关闭，其余任何值都是开启', () => {
    for (const raw of ['false', '0', 'off', 'no', '  false  ']) {
      expect(autoRefreshEnabledFromSettings({ [SETTING_KEY_AUTO_REFRESH_ENABLED]: raw })).toBe(
        false,
      )
    }
    for (const raw of ['true', '1', 'on', 'yes', '', 'anything']) {
      expect(autoRefreshEnabledFromSettings({ [SETTING_KEY_AUTO_REFRESH_ENABLED]: raw })).toBe(true)
    }
  })

  it('大小写敏感，与 Rust 的 matches! 分支一致：False 不算关闭', () => {
    expect(autoRefreshEnabledFromSettings({ [SETTING_KEY_AUTO_REFRESH_ENABLED]: 'False' })).toBe(
      true,
    )
  })
})

describe('settingsKeys/autoUpdateEnabledFromSettings', () => {
  it('键缺失视为开启，与 Rust 的自动安装缺省策略一致', () => {
    expect(autoUpdateEnabledFromSettings({})).toBe(true)
  })

  it('只有 false / 0 / off / no 关闭，其余值保持开启', () => {
    for (const raw of ['false', '0', 'off', 'no', '  no  ']) {
      expect(autoUpdateEnabledFromSettings({ [SETTING_KEY_AUTO_UPDATE_ENABLED]: raw })).toBe(false)
    }
    for (const raw of ['true', '1', 'on', 'yes', '', 'anything', 'False']) {
      expect(autoUpdateEnabledFromSettings({ [SETTING_KEY_AUTO_UPDATE_ENABLED]: raw })).toBe(true)
    }
  })
})

describe('settingsKeys/intervalSecondsFromSettings', () => {
  it('毫秒读成整秒', () => {
    expect(
      intervalSecondsFromSettings(
        { [SETTING_KEY_LOCAL_INTERVAL_MS]: '900000' },
        SETTING_KEY_LOCAL_INTERVAL_MS,
        600,
      ),
    ).toBe(900)
  })

  it('缺失、零、负数、非数字回落到给定缺省值', () => {
    for (const raw of [undefined, '0', '-1', 'abc', '']) {
      expect(
        intervalSecondsFromSettings(
          raw === undefined ? {} : { [SETTING_KEY_LOCAL_INTERVAL_MS]: raw },
          SETTING_KEY_LOCAL_INTERVAL_MS,
          600,
        ),
      ).toBe(600)
    }
  })
})
