import { execFileSync } from 'node:child_process'
import { copyFileSync, mkdirSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const gui = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const root = resolve(gui, '../..')
const release = process.argv.includes('--release')
const profile = release ? 'release' : 'debug'
execFileSync('cargo', ['build', '-p', 'tylora-daemon', ...(release ? ['--release'] : [])], { cwd: root, stdio: 'inherit' })
const host = execFileSync('rustc', ['-vV'], { encoding: 'utf8' }).match(/^host: (.+)$/m)?.[1]
if (!host) throw new Error('Could not determine Rust host triple')
const destination = resolve(gui, `src-tauri/binaries/tylora-${host}`)
mkdirSync(dirname(destination), { recursive: true })
copyFileSync(resolve(root, `target/${profile}/tylora`), destination)
console.log(`Prepared ${destination}`)
