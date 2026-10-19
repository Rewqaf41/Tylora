import { create } from 'zustand'
import { persist } from 'zustand/middleware'
import type { LanguagePref } from '../i18n/lang'

export type Theme = 'system' | 'light' | 'dark'
interface UIState { theme: Theme; setTheme: (theme: Theme) => void; language: LanguagePref; setLanguage: (language: LanguagePref) => void }
export const useUI = create<UIState>()(persist((set) => ({
  theme: 'system', setTheme: (theme) => set({ theme }),
  language: 'system', setLanguage: (language) => set({ language }),
}), { name: 'tylora-ui' }))
