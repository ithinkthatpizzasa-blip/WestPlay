import { describe, expect, it } from 'vitest'
import { HVT_MAP_MESSAGES } from '../../../cabin/maps/blockedMap'
import { offeredKeys } from '../../system'
import { count, languageOf, languages, onMac, type Plural, type Words, wordsFor } from '.'

/** Every count's words in `words`. */
function plurals(words: Words): [string, Plural][] {
  return [
    ['summary.dlcs', words.summary.dlcs],
    ['check.profileCount', words.check.profileCount],
    ['check.unreadable', words.check.unreadable],
  ]
}

/** Every text in `words`, by its path. */
function texts(value: unknown, path = ''): [string, string][] {
  if (typeof value === 'string') return [[path, value]]
  if (value && typeof value === 'object') {
    return Object.entries(value).flatMap(([key, inner]) => texts(inner, path ? `${path}.${key}` : key))
  }
  return []
}

describe('the onboarding’s words', () => {
  it('are there in each of the game’s languages', () => {
    expect(languages()).toEqual(Object.keys(HVT_MAP_MESSAGES).sort())
  })

  it.each(languages())('%s has a form for every count it shows, by its plural rules', (code) => {
    const words = wordsFor(code)
    const rules = new Intl.PluralRules(words.locale)
    for (const [name, plural] of plurals(words)) {
      for (let n = 0; n <= 150; n++) {
        const category = rules.select(n)
        expect(plural[category], `${name} for ${n} (${category})`).toBeDefined()
        expect(count(words, plural, n)).toContain(new Intl.NumberFormat(words.locale).format(n))
      }
    }
  })

  it.each(languages())('%s fills in what it names, and leaves nothing empty', (code) => {
    const words = wordsFor(code)
    for (const [path, text] of texts(words)) expect(text.trim(), path).not.toBe('')
    expect(words.summary.version).toContain('{version}')
    expect(words.check.withMods).toContain('{map}')
    expect(words.check.withMods).toContain('{mods}')
    expect(words.launch.steps).toHaveLength(2)
  })

  it('takes Windows’ language, as the game has it', () => {
    expect(languageOf(['tr-TR'])).toBe('tr_tr')
    expect(languageOf(['en-US'])).toBe('en_us')
    expect(languageOf(['en-GB'])).toBe('en_gb')
    expect(languageOf(['en'])).toBe('en_us')
    expect(languageOf(['de-AT'])).toBe('de_de')
    expect(languageOf(['es-MX'])).toBe('es_la')
    expect(languageOf(['es-ES'])).toBe('es_es')
    expect(languageOf(['fr-CA'])).toBe('fr_ca')
    expect(languageOf(['fr-BE'])).toBe('fr_fr')
    expect(languageOf(['pt-BR'])).toBe('pt_br')
    expect(languageOf(['pt-PT'])).toBe('pt_pt')
    expect(languageOf(['zh-Hant-TW'])).toBe('zh_tw')
    expect(languageOf(['zh-CN'])).toBe('zh_cn')
    expect(languageOf(['sr-Latn-RS'])).toBe('sr_sp')
    expect(languageOf(['sr-Cyrl-RS'])).toBe('sr_sr')
    expect(languageOf(['nb-NO'])).toBe('no_no')
    // A language the game doesn't have: the next one Windows lists, else English.
    expect(languageOf(['xh-ZA', 'de-DE'])).toBe('de_de')
    expect(languageOf(['xh-ZA'])).toBe('en_us')
  })
})

describe('the onboarding’s words on the Mac', () => {
  it.each(languages())('%s names nothing of Windows’, its own words in their place', (code) => {
    const windows = wordsFor(code)
    const before = JSON.stringify(windows)
    const words = onMac(windows)
    // Windows' own are left as they were.
    expect(JSON.stringify(windows)).toBe(before)
    for (const [path, text] of texts({ ...words, mac: null })) expect(text, `${code} ${path}`).not.toMatch(/Windows/)
    // The keys' names ("Alt" is also Danish and Norwegian for "all").
    for (const [path, text] of texts(words.controls)) expect(text, `${code} ${path}`).not.toMatch(/\b(Alt|Ctrl|Strg)\b/)
    const { mac, install, controls } = words
    expect(install.asAdministrator).toBe(mac.asAdministrator)
    expect(install.banners.permission).toEqual({
      title: windows.install.banners.permission.title,
      text: mac.permission,
    })
    expect(install.needsAdministrator.text).toBe(mac.needsAdministrator)
    expect(install.declined.text).toBe(mac.declined)
    expect(install.noDocuments).toBe(mac.noDocuments)
    expect(controls.keys).toEqual({ ...windows.controls.keys, ...mac.keys })
    expect([controls.alt, controls.ctrl]).toEqual(['⌥', '⌃'])
    // Each says it its own way, not Windows' words again.
    const windowsTexts = texts({ ...windows, mac: null }).map(([, text]) => text)
    for (const [path, text] of texts(mac)) expect(windowsTexts, `${code} mac.${path}`).not.toContain(text)
  })
})

describe('the keys offered to hold', () => {
  it('are right Option on the Mac where Windows has Right Ctrl, which most Macs don’t have', () => {
    expect(offeredKeys('windows')).toEqual(['leftAlt', 'rightCtrl', 'mouseBack', 'mouseForward'])
    expect(offeredKeys('mac')).toEqual(['leftAlt', 'rightAlt', 'mouseBack', 'mouseForward'])
  })

  it.each(languages())('%s names each on its system', (code) => {
    const windows = wordsFor(code)
    for (const key of offeredKeys('windows')) expect(windows.controls.keys[key], key).toBeTruthy()
    // Right Alt is Windows' to name there (it isn't offered): as before.
    expect(windows.controls.keys.rightAlt).toBeUndefined()
    const mac = onMac(windows)
    for (const key of offeredKeys('mac')) expect(mac.controls.keys[key], key).toBeTruthy()
  })
})
