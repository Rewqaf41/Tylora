import { tEn, type T } from '../i18n'

export function shortcutFromCode(code: string, modifiers: { meta: boolean; ctrl: boolean; alt: boolean; shift: boolean }): string | null {
  const key = code.startsWith('Key') && code.length === 4 ? code.slice(3).toLowerCase()
    : code.startsWith('Digit') && code.length === 6 ? code.slice(5)
    : code.startsWith('Arrow') ? code.slice(5).toLowerCase()
    : /^F([1-9]|1[0-2])$/.test(code) ? code.toLowerCase()
    : code === 'Tab' ? 'tab'
    : code === 'Backquote' ? 'grave'
    : null
  if (!key || !(modifiers.meta || modifiers.ctrl || modifiers.alt || modifiers.shift)) return null
  return [modifiers.meta && 'command', modifiers.ctrl && 'control', modifiers.alt && 'option', modifiers.shift && 'shift', key].filter(Boolean).join('+')
}

export function shortcutWarning(chord: string, t: T = tEn): string | null {
  if (['command+q', 'command+w', 'command+tab', 'command+space', 'control+up', 'control+down'].includes(chord))
    return t('recorder.reserved')
  return null
}

type Modifiers = { meta: boolean; ctrl: boolean; alt: boolean; shift: boolean }

/** Held modifiers as the recorder shows them while waiting for a key. */
export function heldModifiers(modifiers: Modifiers): string {
  return [modifiers.meta && 'command', modifiers.ctrl && 'control', modifiers.alt && 'option', modifiers.shift && 'shift'].filter(Boolean).join('+')
}

/** Why a key press was not recorded, or null when it was a lone modifier. */
export function rejectionReason(code: string, modifiers: Modifiers, t: T = tEn): string | null {
  if (/^(Meta|Control|Alt|Shift)(Left|Right)$/.test(code) || code === 'CapsLock' || code === 'Fn') return null
  if (!(modifiers.meta || modifiers.ctrl || modifiers.alt || modifiers.shift)) return t('recorder.needModifier')
  return t('recorder.unsupported', { key: code.replace(/^Key|^Digit/, '') })
}
