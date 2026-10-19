import { describe, expect, it } from 'vitest'
import { shortcutFromCode, shortcutWarning } from './hotkey'

describe('physical hotkey recorder', () => {
  it('uses physical digit position despite Option character', () => {
    expect(shortcutFromCode('Digit1', { meta: false, ctrl: false, alt: true, shift: true })).toBe('option+shift+1')
  })
  it('accepts arrows and function keys with modifiers', () => {
    expect(shortcutFromCode('ArrowLeft', { meta: false, ctrl: true, alt: false, shift: false })).toBe('control+left')
    expect(shortcutFromCode('F12', { meta: true, ctrl: false, alt: false, shift: false })).toBe('command+f12')
  })
  it('rejects unmodified and unsupported keys', () => {
    expect(shortcutFromCode('KeyA', { meta: false, ctrl: false, alt: false, shift: false })).toBeNull()
    expect(shortcutFromCode('Numpad1', { meta: false, ctrl: false, alt: true, shift: false })).toBeNull()
  })
  it('warns about known system shortcuts without claiming exhaustive detection', () => {
    expect(shortcutWarning('command+q')).toMatch(/macOS/)
    expect(shortcutWarning('option+shift+1')).toBeNull()
  })
})
