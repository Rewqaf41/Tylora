import { create } from 'zustand'
import { persist } from 'zustand/middleware'

export type Theme = 'system' | 'light' | 'dark'
interface UIState { theme: Theme; setTheme: (theme: Theme) => void }
export const useUI = create<UIState>()(persist((set) => ({
  theme: 'system', setTheme: (theme) => set({ theme }),
}), { name: 'tylora-ui' }))
