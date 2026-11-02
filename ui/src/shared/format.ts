const DAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun']

function minuteOfDay(gameTime: number): number {
  return ((gameTime % 1440) + 1440) % 1440
}

/** "14:22" — the clock as shown in the sidebar. */
export function formatGameTime(gameTime: number): string {
  const minutes = minuteOfDay(gameTime)
  return `${String(Math.floor(minutes / 60)).padStart(2, '0')}:${String(minutes % 60).padStart(2, '0')}`
}

/** "Thu 14:22" */
export function formatGameClock(gameTime: number): string {
  return `${DAYS[Math.floor(gameTime / 1440) % 7]} ${formatGameTime(gameTime)}`
}

/** "3", "N", "R1" */
export function formatGear(displayedGear: number): string {
  if (displayedGear > 0) return String(displayedGear)
  if (displayedGear < 0) return `R${-displayedGear}`
  return 'N'
}

/** Metres in a mile, and in a yard. */
const MILE = 1609.344
const YARD = 0.9144

/** Truck speed (m/s, negative when reversing) as whole km/h, or mph with the game's units imperial. */
export function wholeSpeed(speed: number, miles = false): number {
  return Math.round(Math.abs(speed) * (miles ? 3600 / MILE : 3.6))
}

/** A distance in whole kilometres (as a job plans it) as shown: "1214 km", "754 mi". */
export function formatKilometres(km: number, miles = false): string {
  return miles ? `${Math.round((km * 1000) / MILE)} mi` : `${Math.round(km)} km`
}

/** How a trip's time is said: in minutes, or hours and minutes ("12 min", "1:05 hrs"). */
export interface TimeWords {
  minutes: (minutes: number) => string
  hours: (hours: number, minutes: string) => string
}

/** A trip's time left (game minutes) as Apple shows it: "12 min", "1:05 hrs". */
export function formatTripTime(minutes: number, words: TimeWords): string {
  const whole = Math.max(0, Math.round(minutes))
  if (whole < 60) return words.minutes(whole)
  return words.hours(Math.floor(whole / 60), String(whole % 60).padStart(2, '0'))
}

/** A distance (metres, as the game counts them) as Apple shows it: "850 m", "9.4 km", "214 km" (with the
 * language's decimal separator). Close by, to the nearest 10 m, from 100 m the nearest 50. With the game's
 * units imperial (`miles`), in yards then miles as the game's own HUD has them: "440 yd", "0.3 mi", "133 mi". */
export function formatTripDistance(metres: number, decimal = '.', miles = false): string {
  if (miles) return formatImperial(metres, decimal)
  if (metres < 100) return `${Math.max(0, Math.round(metres / 10) * 10)} m`
  if (metres < 975) return `${Math.round(metres / 50) * 50} m`
  if (metres < 9950) return `${(metres / 1000).toFixed(1).replace('.', decimal)} km`
  return `${Math.round(metres / 1000)} km`
}

/** Yards close by (to the nearest 10, from 100 yd the nearest 50), from a quarter mile in miles: tenths
 * below 10 mi, then whole. */
function formatImperial(metres: number, decimal: string): string {
  const yards = metres / YARD
  if (yards < 100) return `${Math.max(0, Math.round(yards / 10) * 10)} yd`
  const mi = metres / MILE
  if (mi < 0.25) return `${Math.round(yards / 50) * 50} yd`
  if (mi < 9.95) return `${mi.toFixed(1).replace('.', decimal)} mi`
  return `${Math.round(mi)} mi`
}
