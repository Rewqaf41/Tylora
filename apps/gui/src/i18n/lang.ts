export type Lang = 'en' | 'ru'
export type LanguagePref = 'system' | Lang

/** The UI language: the saved choice, else the macOS language (Russian or English). */
export function resolveLang(pref: LanguagePref, system = typeof navigator === 'undefined' ? 'en' : navigator.language): Lang {
  if (pref !== 'system') return pref
  return system.toLowerCase().startsWith('ru') ? 'ru' : 'en'
}
