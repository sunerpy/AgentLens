#!/usr/bin/env node

import { readFile, readdir, writeFile } from 'node:fs/promises'
import { basename, join, resolve } from 'node:path'

const PLATFORM_ASSETS = [
  { key: 'windows-x86_64-nsis', suffix: '-setup.exe' },
  { key: 'windows-x86_64-msi', suffix: '.msi' },
  { key: 'linux-x86_64-deb', suffix: '.deb' },
  { key: 'darwin-aarch64', suffix: '.app.tar.gz' },
]

function parseArgs(argv) {
  const parsed = {}
  for (let index = 0; index < argv.length; index += 2) {
    const flag = argv[index]
    const value = argv[index + 1]
    if (!flag?.startsWith('--') || value === undefined) {
      throw new Error(`invalid arguments near ${flag ?? '<end>'}`)
    }
    parsed[flag.slice(2)] = value
  }
  return parsed
}

function required(args, key) {
  const value = args[key]
  if (typeof value !== 'string' || value.trim() === '') {
    throw new Error(`missing required argument --${key}`)
  }
  return value
}

function releaseAssetUrl(repository, tag, name) {
  return `https://github.com/${repository}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(name)}`
}

async function generate() {
  const args = parseArgs(process.argv.slice(2))
  const assetsDirectory = resolve(required(args, 'assets'))
  const version = required(args, 'version')
  const tag = required(args, 'tag')
  const repository = required(args, 'repository')
  const notesPath = resolve(required(args, 'notes'))
  const pubDate = required(args, 'pub-date')
  const outputPath = resolve(args.output ?? join(assetsDirectory, 'latest.json'))

  if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/.test(version)) {
    throw new Error(`invalid semantic version: ${version}`)
  }
  if (!/^[^/\s]+\/[^/\s]+$/.test(repository)) {
    throw new Error(`invalid GitHub repository: ${repository}`)
  }
  if (Number.isNaN(Date.parse(pubDate))) {
    throw new Error(`invalid RFC 3339 publication date: ${pubDate}`)
  }

  const names = await readdir(assetsDirectory)
  const platforms = {}
  for (const definition of PLATFORM_ASSETS) {
    const matches = names.filter(
      (name) => name.endsWith(definition.suffix) && !name.endsWith(`${definition.suffix}.sig`),
    )
    if (matches.length !== 1) {
      throw new Error(
        `${definition.key}: expected exactly one *${definition.suffix} asset, found ${matches.length}${
          matches.length === 0 ? '' : ` (${matches.join(', ')})`
        }`,
      )
    }

    const assetName = matches[0]
    const signatureName = `${assetName}.sig`
    if (!names.includes(signatureName)) {
      throw new Error(`${definition.key}: missing signature ${signatureName}`)
    }
    const signature = (await readFile(join(assetsDirectory, signatureName), 'utf8')).trim()
    if (signature === '') {
      throw new Error(`${definition.key}: empty signature ${signatureName}`)
    }
    platforms[definition.key] = {
      signature,
      url: releaseAssetUrl(repository, tag, assetName),
    }
  }

  const manifest = {
    version,
    notes: await readFile(notesPath, 'utf8'),
    pub_date: pubDate,
    platforms,
  }
  await writeFile(outputPath, `${JSON.stringify(manifest, null, 2)}\n`, 'utf8')
  process.stdout.write(`wrote ${basename(outputPath)} with ${Object.keys(platforms).length} platforms\n`)
}

generate().catch((error) => {
  process.stderr.write(`generate-latest-json: ${error instanceof Error ? error.message : String(error)}\n`)
  process.exitCode = 1
})
