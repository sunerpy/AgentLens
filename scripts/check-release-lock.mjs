#!/usr/bin/env node
/**
 * Guards the release-please <-> Cargo.lock version-bump contract.
 *
 * `release-please-config.json` bumps `Cargo.lock` through an `extra-files` entry whose
 * jsonpath names the workspace crates explicitly:
 *
 *   $.package[?(['a','b'].includes(@.name.value))].version
 *
 * That entry has a silent failure mode. When a new workspace member is added and nobody
 * adds it to the list, release-please only prints `No entries modified` and still exits 0 —
 * so CI stays green while the lockfile drifts one version behind on every release, and any
 * later `cargo` invocation rewrites it, producing phantom diffs in a clean worktree. This
 * script turns that silence into a hard failure by asserting three-way parity:
 *
 *   1. the crate names in the jsonpath list
 *   2. the actual workspace members reported by `cargo metadata`
 *   3. the `[[package]]` entries present in `Cargo.lock`
 *
 * It also pins the jsonpath *shape*, because two other spellings fail silently or too
 * loudly:
 *   - `@.name` (without `.value`) matches nothing: release-please parses TOML with a
 *     tagged parser that wraps every scalar in `{__TAGGED_VALUE, start, end, value}`,
 *     so the bare `@.name` is an object, never a string.
 *   - a wide match such as `$..version` would also rewrite the `version = 3` on line 3 of
 *     Cargo.lock, which is the lockfile *format* version, not a crate version.
 *
 * Exit codes: 0 clean, 1 parity violation, 2 unusable input.
 *
 * Usage: node scripts/check-release-lock.mjs [repoRoot]
 */
import { execFileSync } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import process from 'node:process'

const repoRoot = path.resolve(process.argv[2] ?? path.join(import.meta.dirname, '..'))
const configPath = path.join(repoRoot, 'release-please-config.json')
const lockPath = path.join(repoRoot, 'Cargo.lock')

/** The jsonpath shape the Cargo.lock entry must keep; see the header for why. */
const LIST_SHAPE = /\[((?:\s*'[^']+'\s*,?)+)\]\s*\.includes\(\s*@\.name\.value\s*\)/

function fail(message) {
  process.stderr.write(`check-release-lock: ${message}\n`)
  process.exit(2)
}

function readJson(absolute) {
  try {
    return JSON.parse(fs.readFileSync(absolute, 'utf8'))
  } catch (error) {
    fail(`unable to read ${path.relative(repoRoot, absolute)}: ${error.message}`)
  }
}

/** Crate names listed in the Cargo.lock extra-files jsonpath. */
function namesFromConfig() {
  const config = readJson(configPath)
  const pkg = config?.packages?.['.']
  if (!pkg) {
    fail('release-please-config.json has no `packages["."]` entry')
  }
  const entries = Array.isArray(pkg['extra-files']) ? pkg['extra-files'] : []
  const entry = entries.find((candidate) => candidate?.path === 'Cargo.lock')
  if (!entry) {
    fail(
      'release-please-config.json has no `extra-files` entry for Cargo.lock, so releases will ' +
        'not bump the lockfile. Add one of type "toml" whose jsonpath lists the workspace crates.',
    )
  }
  if (entry.type !== 'toml') {
    fail(`the Cargo.lock extra-files entry must be of type "toml", found "${entry.type}"`)
  }
  const jsonpath = typeof entry.jsonpath === 'string' ? entry.jsonpath : ''
  if (!jsonpath.startsWith('$.package[')) {
    fail(
      `the Cargo.lock jsonpath must be anchored at "$.package[" so it cannot reach the ` +
        `lockfile format version on line 3; found: ${jsonpath}`,
    )
  }
  if (!jsonpath.endsWith('].version')) {
    fail(`the Cargo.lock jsonpath must select "].version"; found: ${jsonpath}`)
  }
  const match = LIST_SHAPE.exec(jsonpath)
  if (!match) {
    fail(
      'the Cargo.lock jsonpath must filter on an explicit quoted crate list against ' +
        "@.name.value, e.g. $.package[?(['crate-a','crate-b'].includes(@.name.value))].version — " +
        `a bare @.name silently matches nothing. Found: ${jsonpath}`,
    )
  }
  return match[1]
    .split(',')
    .map((raw) => raw.trim().replace(/^'|'$/g, ''))
    .filter((name) => name !== '')
}

/** Workspace member package names, straight from cargo so globs and renames cannot drift. */
function namesFromCargo() {
  let raw
  try {
    // Deliberately no --locked: the lockfile is expected to lag by one version right after a
    // release, which is the very condition this guardrail exists to prevent recurring — it
    // must not turn that lag into its own failure. --no-deps keeps it to workspace members.
    raw = execFileSync('cargo', ['metadata', '--no-deps', '--format-version', '1'], {
      cwd: repoRoot,
      encoding: 'utf8',
      maxBuffer: 64 * 1024 * 1024,
      stdio: ['ignore', 'pipe', 'pipe'],
    })
  } catch (error) {
    fail(`\`cargo metadata\` failed: ${error.message}`)
  }
  let meta
  try {
    meta = JSON.parse(raw)
  } catch (error) {
    fail(`unable to parse \`cargo metadata\` output: ${error.message}`)
  }
  const members = new Set(meta.workspace_members ?? [])
  const names = (meta.packages ?? [])
    .filter((entry) => members.has(entry.id))
    .map((entry) => entry.name)
  if (names.length === 0) {
    fail('`cargo metadata` reported no workspace members')
  }
  return names
}

/** Package names that actually have a `[[package]]` block in Cargo.lock. */
function namesInLock() {
  let text
  try {
    text = fs.readFileSync(lockPath, 'utf8')
  } catch (error) {
    fail(`unable to read Cargo.lock: ${error.message}`)
  }
  const names = new Set()
  let inPackage = false
  for (const line of text.split('\n')) {
    const trimmed = line.trim()
    if (trimmed === '[[package]]') {
      inPackage = true
      continue
    }
    if (trimmed.startsWith('[')) {
      inPackage = false
      continue
    }
    if (!inPackage) continue
    const named = /^name = "([^"]+)"$/.exec(trimmed)
    if (named) names.add(named[1])
  }
  return names
}

function sorted(values) {
  return [...values].sort()
}

const listed = namesFromConfig()
const workspace = namesFromCargo()
const lock = namesInLock()

const listedSet = new Set(listed)
const workspaceSet = new Set(workspace)

const missing = sorted(workspace.filter((name) => !listedSet.has(name)))
const extra = sorted(listed.filter((name) => !workspaceSet.has(name)))
const absentFromLock = sorted(listed.filter((name) => !lock.has(name)))
const duplicated = sorted(listed.filter((name, index) => listed.indexOf(name) !== index))

if (missing.length + extra.length + absentFromLock.length + duplicated.length > 0) {
  process.stderr.write(
    'check-release-lock: the Cargo.lock crate list in release-please-config.json is out of sync.\n',
  )
  if (missing.length > 0) {
    process.stderr.write(
      `  missing from the list (workspace members release-please would skip): ${missing.join(', ')}\n`,
    )
  }
  if (extra.length > 0) {
    process.stderr.write(
      `  stale in the list (no longer a workspace member): ${extra.join(', ')}\n`,
    )
  }
  if (absentFromLock.length > 0) {
    process.stderr.write(
      `  listed but absent from Cargo.lock (renamed or never locked): ${absentFromLock.join(', ')}\n`,
    )
  }
  if (duplicated.length > 0) {
    process.stderr.write(`  listed more than once: ${duplicated.join(', ')}\n`)
  }
  process.stderr.write(
    '\nWhy this is fatal: release-please only warns `No entries modified` for a crate it cannot\n' +
      'find, exits 0, and ships a release whose Cargo.lock still carries the previous version.\n' +
      'The drift then surfaces as phantom diffs the next time anyone runs cargo.\n' +
      'Fix: edit the `jsonpath` of the Cargo.lock entry in release-please-config.json so its\n' +
      `quoted crate list is exactly the workspace members: ${sorted(workspace).join(', ')}\n`,
  )
  process.exit(1)
}

process.stdout.write(
  `check-release-lock: OK — ${listed.length} crate(s) bumped in Cargo.lock ` +
    `(${sorted(listed).join(', ')}), matching the workspace and the lockfile.\n`,
)
