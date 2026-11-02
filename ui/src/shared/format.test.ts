import { describe, expect, it } from 'vitest'
import { formatKilometres, formatTripDistance, formatTripTime, wholeSpeed } from './format'

const words = { minutes: (m: number) => `${m} min`, hours: (h: number, m: string) => `${h}:${m} hrs` }

describe('trip', () => {
  it('shows the time left in minutes, then hours and minutes', () => {
    expect(formatTripTime(0.4, words)).toBe('0 min')
    expect(formatTripTime(12.2, words)).toBe('12 min')
    expect(formatTripTime(59.4, words)).toBe('59 min')
    expect(formatTripTime(65, words)).toBe('1:05 hrs')
    expect(formatTripTime(754, words)).toBe('12:34 hrs')
  })

  it('shows a distance in metres close by, then kilometres, in the language’s decimals', () => {
    expect(formatTripDistance(42)).toBe('40 m')
    expect(formatTripDistance(830)).toBe('850 m')
    expect(formatTripDistance(990)).toBe('1.0 km')
    expect(formatTripDistance(9420)).toBe('9.4 km')
    expect(formatTripDistance(9420, ',')).toBe('9,4 km')
    expect(formatTripDistance(67_703)).toBe('68 km')
  })

  it('shows a distance in yards close by, then miles, with the game’s units imperial', () => {
    expect(formatTripDistance(42, '.', true)).toBe('50 yd')
    expect(formatTripDistance(300, '.', true)).toBe('350 yd')
    expect(formatTripDistance(400, '.', true)).toBe('450 yd')
    expect(formatTripDistance(500, '.', true)).toBe('0.3 mi')
    expect(formatTripDistance(9420, '.', true)).toBe('5.9 mi')
    expect(formatTripDistance(9420, ',', true)).toBe('5,9 mi')
    expect(formatTripDistance(16_000, '.', true)).toBe('9.9 mi')
    expect(formatTripDistance(16_020, '.', true)).toBe('10 mi')
    expect(formatTripDistance(67_703, '.', true)).toBe('42 mi')
  })

  it('shows a job’s planned distance in km, or miles', () => {
    expect(formatKilometres(1480)).toBe('1480 km')
    expect(formatKilometres(1480, true)).toBe('920 mi')
  })
})

describe('speed', () => {
  it('is whole km/h, or mph, either way the truck goes', () => {
    expect(wholeSpeed(25)).toBe(90)
    expect(wholeSpeed(25, true)).toBe(56)
    expect(wholeSpeed(-13.9)).toBe(50)
    expect(wholeSpeed(-13.9, true)).toBe(31)
  })
})
