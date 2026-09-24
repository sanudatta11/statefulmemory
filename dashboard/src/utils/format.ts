const numberFormatter = new Intl.NumberFormat('en-US')
const compactFormatter = new Intl.NumberFormat('en-US', {
  notation: 'compact',
  maximumFractionDigits: 1,
})
const dateFormatter = new Intl.DateTimeFormat('en-US', {
  month: 'short',
  day: 'numeric',
  year: 'numeric',
})
const timeFormatter = new Intl.DateTimeFormat('en-US', {
  hour: '2-digit',
  minute: '2-digit',
})

export function formatNumber(value: number): string {
  return numberFormatter.format(value)
}

export function formatCompact(value: number): string {
  return compactFormatter.format(value)
}

export function formatDate(value: string): string {
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? 'Unknown date' : dateFormatter.format(date)
}

export function formatTime(value: string): string {
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? '--:--' : timeFormatter.format(date)
}

export function formatRelativeTime(value: string): string {
  const timestamp = new Date(value).getTime()
  if (Number.isNaN(timestamp)) return 'unknown'
  const deltaMinutes = Math.round((timestamp - Date.now()) / 60_000)
  if (Math.abs(deltaMinutes) < 1) return 'just now'
  if (Math.abs(deltaMinutes) < 60) return `${Math.abs(deltaMinutes)}m ago`
  const deltaHours = Math.round(deltaMinutes / 60)
  if (Math.abs(deltaHours) < 24) return `${Math.abs(deltaHours)}h ago`
  const deltaDays = Math.round(deltaHours / 24)
  if (Math.abs(deltaDays) < 7) return `${Math.abs(deltaDays)}d ago`
  return formatDate(value)
}
