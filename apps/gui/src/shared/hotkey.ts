export function shortcutFromCode(code: string, modifiers: { meta: boolean; ctrl: boolean; alt: boolean; shift: boolean }): string | null {
  const key = code.startsWith('Key') && code.length === 4 ? code.slice(3).toLowerCase()
    : code.startsWith('Digit') && code.length === 6 ? code.slice(5)
    : code.startsWith('Arrow') ? code.slice(5).toLowerCase()
    : /^F([1-9]|1[0-2])$/.test(code) ? code.toLowerCase()
    : code === 'Tab' ? 'tab'
    : null
  if (!key || !(modifiers.meta || modifiers.ctrl || modifiers.alt || modifiers.shift)) return null
  return [modifiers.meta && 'command', modifiers.ctrl && 'control', modifiers.alt && 'option', modifiers.shift && 'shift', key].filter(Boolean).join('+')
}

export function shortcutWarning(chord: string): string | null {
  if (['command+q', 'command+w', 'command+tab', 'command+space', 'control+up', 'control+down'].includes(chord))
    return 'This combination is commonly reserved by macOS or an app. Tylora cannot detect every system shortcut conflict.'
  return null
}
