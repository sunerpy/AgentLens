#!/usr/bin/env node
/**
 * Guards the single-dictionary rule: no user-visible Chinese string may be hard-coded in a
 * frontend component. Every such string must come from `frontend/src/i18n/zh.ts`.
 *
 * Uses the TypeScript compiler's own AST (already a devDependency) rather than regexes, so
 * comments, JSX text, template literals and nested strings are classified exactly.
 *
 * Exit codes: 0 clean, 1 violations found, 2 unusable input.
 *
 * Usage: node scripts/check-i18n.mjs [repoRoot]
 */
import { createRequire } from 'node:module'
import fs from 'node:fs'
import path from 'node:path'
import process from 'node:process'

const repoRoot = path.resolve(process.argv[2] ?? path.join(import.meta.dirname, '..'))
const srcRoot = path.join(repoRoot, 'frontend', 'src')

/**
 * Paths exempt from the rule, each for a stated reason:
 * - `src/i18n/**` is the dictionary itself.
 * - `src/generated/**` is emitted by the Rust ts-rs gate and must never be hand-edited.
 * - `src/lib/mockIpc.ts` holds fixture data that mirrors Chinese strings the backend
 *   produces at runtime (e.g. SSH remediation text); they are data, not UI copy.
 */
const ALLOWED = [
  path.join('src', 'i18n'),
  path.join('src', 'generated'),
  path.join('src', 'lib', 'mockIpc.ts'),
]

/**
 * Unit-test files (`*.test.ts` / `*.spec.tsx`, run by Vitest) are exempt: their Chinese
 * strings are `describe`/`it` descriptions and expected-value fixtures — developer-facing
 * test names, never rendered UI copy. Routing them through the zh dictionary would couple
 * the suite to the UI vocabulary and make failures harder to read.
 */
const TEST_FILE = /\.(test|spec)\.tsx?$/

const CJK = /[\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff\u3040-\u30ff]/

function fail(message) {
  process.stderr.write(`check-i18n: ${message}\n`)
  process.exit(2)
}

if (!fs.existsSync(srcRoot)) {
  fail(`frontend source directory not found: ${srcRoot}`)
}

let ts
try {
  const require = createRequire(path.join(repoRoot, 'frontend', 'package.json'))
  ts = require('typescript')
} catch (error) {
  fail(`unable to load the typescript compiler from frontend/node_modules: ${error.message}`)
}
if (typeof ts?.createSourceFile !== 'function') {
  fail('the resolved typescript package does not expose createSourceFile')
}

function collectFiles(dir) {
  const found = []
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const absolute = path.join(dir, entry.name)
    const relative = path.relative(path.join(repoRoot, 'frontend'), absolute)
    if (ALLOWED.some((allowed) => relative === allowed || relative.startsWith(`${allowed}${path.sep}`))) {
      continue
    }
    if (entry.isDirectory()) {
      found.push(...collectFiles(absolute))
    } else if (/\.(ts|tsx)$/.test(entry.name) && !TEST_FILE.test(entry.name)) {
      found.push(absolute)
    }
  }
  return found
}

function lineOf(sourceFile, position) {
  return sourceFile.getLineAndCharacterOfPosition(position).line + 1
}

function violationsIn(absolute) {
  const text = fs.readFileSync(absolute, 'utf8')
  const sourceFile = ts.createSourceFile(
    absolute,
    text,
    ts.ScriptTarget.ESNext,
    true,
    absolute.endsWith('.tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.TS,
  )
  const found = []

  const record = (node, literal) => {
    const trimmed = literal.trim()
    if (trimmed !== '' && CJK.test(trimmed)) {
      found.push({ line: lineOf(sourceFile, node.getStart(sourceFile)), literal: trimmed })
    }
  }

  const walk = (node) => {
    if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) {
      record(node, node.text)
    } else if (ts.isTemplateHead(node) || ts.isTemplateMiddle(node) || ts.isTemplateTail(node)) {
      record(node, node.text)
    } else if (ts.isJsxText(node)) {
      record(node, node.text)
    }
    ts.forEachChild(node, walk)
  }

  walk(sourceFile)
  return found
}

const files = collectFiles(srcRoot).sort()
let total = 0
for (const absolute of files) {
  for (const violation of violationsIn(absolute)) {
    total += 1
    const relative = path.relative(repoRoot, absolute)
    process.stdout.write(`${relative}:${violation.line}: bare Chinese literal: ${violation.literal}\n`)
  }
}

if (total > 0) {
  process.stdout.write(
    `\ncheck-i18n: ${total} bare Chinese literal(s) in ${files.length} scanned file(s).\n` +
      'Move the text into frontend/src/i18n/zh.ts and reference it through the zh dictionary.\n',
  )
  process.exit(1)
}

process.stdout.write(`check-i18n: OK — ${files.length} file(s) scanned, no bare Chinese literals.\n`)
