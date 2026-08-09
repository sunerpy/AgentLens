import path from 'node:path'
import { defineConfig } from 'vitest/config'

/**
 * 前端单元测试（Vitest 4）。与另外两层测试严格互斥，三层各有独立入口：
 *
 * | 层 | 入口 | 配置 | 位置 |
 * | --- | --- | --- | --- |
 * | 单元 | `npm run test:unit` | 本文件 | `src/**\/*.{test,spec}.{ts,tsx}` |
 * | 组件级功能 | `npm run test:e2e` | playwright.config.ts | `e2e/` |
 * | 真实驱动 | `npm run test:e2e:real` | wdio.conf.mjs | `e2e-real/` |
 *
 * `include` 只收 `src/` 下的用例，并显式排除 `e2e/` 与 `e2e-real/`：Playwright 的
 * `*.spec.ts` 与 Vitest 默认 include 模式完全同形，一旦放开就会被 Vitest 抓走，
 * 然后在没有 Playwright fixture 的环境里报一堆无意义的失败。**不要往 include 里
 * 加 e2e 相关 glob。**
 *
 * 覆盖率产物落 `../artifacts/coverage/frontend/`（artifacts/ 已被 .gitignore 忽略），
 * 与 Rust 侧 `make coverage` 的 `artifacts/coverage/lcov.info` 同处一棵树，便于 CI
 * 一次性上传给 Codecov。
 */
export default defineConfig({
  resolve: {
    // 与 vite.config.ts / tsconfig 的 `@/*` 别名保持一致，否则单测里 import '@/lib/...' 解析不到。
    alias: {
      '@': path.resolve(import.meta.dirname, './src'),
    },
  },
  test: {
    // jsdom：React 组件单测需要 DOM。纯函数用例不受影响。
    environment: 'jsdom',
    globals: false,
    include: ['src/**/*.{test,spec}.{ts,tsx}'],
    exclude: ['e2e/**', 'e2e-real/**', 'node_modules/**', 'dist/**', 'src/generated/**'],
    coverage: {
      provider: 'v8',
      reporter: ['text', 'lcov'],
      reportsDirectory: '../artifacts/coverage/frontend',
      include: ['src/**/*.{ts,tsx}'],
      // 生成物（ts-rs 产出）、样板入口与用例自身不计入分母。
      exclude: [
        'src/generated/**',
        'src/main.tsx',
        'src/**/*.d.ts',
        'src/**/*.{test,spec}.{ts,tsx}',
      ],
    },
  },
})
