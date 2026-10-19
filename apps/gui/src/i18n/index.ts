import { useMemo } from 'react'
import { useUI } from '../app/store'
import { en, type Messages } from './en'
import { resolveLang, type Lang } from './lang'
import { ru } from './ru'

export { resolveLang, type Lang, type LanguagePref } from './lang'
export type MessageKey = keyof Messages
export type Vars = Record<string, string | number>
export type T = (key: MessageKey, vars?: Vars) => string

const dictionaries: Record<Lang, Messages> = { en, ru }
const PLURAL_ORDER: Record<Lang, string[]> = { en: ['one', 'other'], ru: ['one', 'few', 'many'] }

/**
 * Fills `{name}` placeholders. `{count|# window|# windows}` picks a plural
 * form for the number in `count` (English: one|other, Russian: one|few|many);
 * `#` stands for the number.
 */
export function format(lang: Lang, template: string, vars: Vars = {}): string {
  return template.replace(/\{(\w+)((?:\|[^|}]*)+)?\}/g, (match, name: string, forms?: string) => {
    const value = vars[name]
    if (value === undefined) return match
    if (!forms) return String(value)
    const options = forms.slice(1).split('|')
    const category = new Intl.PluralRules(lang).select(Number(value))
    const index = PLURAL_ORDER[lang].indexOf(category === 'other' && lang === 'ru' ? 'many' : category)
    return (options[index < 0 ? options.length - 1 : index] ?? '').replaceAll('#', String(value))
  })
}

export function translator(lang: Lang): T {
  return (key, vars) => format(lang, dictionaries[lang][key], vars)
}

/** English, for pure helpers called without a component (and in tests). */
export const tEn = translator('en')

export function useLang(): Lang {
  return resolveLang(useUI((state) => state.language))
}

export function useT(): T {
  const lang = useLang()
  return useMemo(() => translator(lang), [lang])
}
