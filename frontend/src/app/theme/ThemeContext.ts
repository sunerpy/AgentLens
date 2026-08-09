import { createContext, useContext } from 'react'

import { DEFAULT_THEME, type ThemeKey } from './themes'

export interface ThemeContextValue {
  theme: ThemeKey
  /** Applies immediately, then persists to `app_settings`; a failed write surfaces as `error`. */
  setTheme: (theme: ThemeKey) => void
  isSaving: boolean
  error: unknown
}

export const ThemeContext = createContext<ThemeContextValue>({
  theme: DEFAULT_THEME,
  setTheme: () => {},
  isSaving: false,
  error: null,
})

export function useTheme(): ThemeContextValue {
  return useContext(ThemeContext)
}
