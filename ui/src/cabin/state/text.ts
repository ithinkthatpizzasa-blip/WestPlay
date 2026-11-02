import { useCallback } from 'react'
import { formatTripDistance } from '../../shared/format'
import { type Language, languageOf, STRINGS, type Strings } from './strings'
import { useLive } from './store'

export type { Language, Strings } from './strings'

/** The cabin's language: the game's. */
export function useLanguage(): Language {
  return languageOf(useLive((state) => state.gameLanguage))
}

/** The cabin's words, in the game's language. */
export function useText(): Strings {
  return STRINGS[languageOf(useLive((state) => state.gameLanguage))]
}

/** How the cabin says a distance (metres): in the game's units (km, or miles with its units imperial), with
 * the language's decimals. */
export function useDistanceText(): (metres: number) => string {
  const text = useText()
  const miles = useLive((state) => state.miles)
  return useCallback((metres: number) => formatTripDistance(metres, text.decimal, miles), [text, miles])
}
