import type { ReactNode } from 'react'

export type IconName =
  | 'activity'
  | 'archive'
  | 'arrow-up-right'
  | 'box'
  | 'check'
  | 'chevron-down'
  | 'chevron-right'
  | 'clock'
  | 'close'
  | 'command'
  | 'copy'
  | 'database'
  | 'external'
  | 'file'
  | 'filter'
  | 'flask'
  | 'graph'
  | 'grid'
  | 'hard-drive'
  | 'info'
  | 'link'
  | 'menu'
  | 'memory'
  | 'more'
  | 'plus'
  | 'play'
  | 'refresh'
  | 'search'
  | 'server'
  | 'shield'
  | 'sliders'
  | 'spark'
  | 'target'
  | 'trend'
  | 'warning'
  | 'x'

interface IconProps {
  name: IconName
  size?: number
  strokeWidth?: number
  className?: string
}

const paths: Record<IconName, ReactNode> = {
  activity: (
    <>
      <path d="M3 12h4l2.2-7 4.1 14L16 9l2 3h3" />
    </>
  ),
  archive: (
    <>
      <path d="M4 7h16v12H4z" />
      <path d="M3 4h18v3H3zM9 11h6" />
    </>
  ),
  'arrow-up-right': <path d="M7 17 17 7M8 7h9v9" />,
  box: (
    <>
      <path d="m4 7 8-4 8 4v10l-8 4-8-4z" />
      <path d="m4 7 8 4 8-4M12 11v10" />
    </>
  ),
  check: <path d="m5 12 4 4L19 6" />,
  'chevron-down': <path d="m6 9 6 6 6-6" />,
  'chevron-right': <path d="m9 6 6 6-6 6" />,
  clock: (
    <>
      <circle cx="12" cy="12" r="8.5" />
      <path d="M12 7v5l3 2" />
    </>
  ),
  close: <path d="m6 6 12 12M18 6 6 18" />,
  command: (
    <>
      <path d="M18 8a3 3 0 1 0-3-3v14a3 3 0 1 0 3-3H6a3 3 0 1 0 3 3V5a3 3 0 1 0-3 3z" />
    </>
  ),
  copy: (
    <>
      <rect x="8" y="8" width="11" height="12" rx="1.5" />
      <path d="M16 8V5.5A1.5 1.5 0 0 0 14.5 4h-9A1.5 1.5 0 0 0 4 5.5v10A1.5 1.5 0 0 0 5.5 17H8" />
    </>
  ),
  database: (
    <>
      <ellipse cx="12" cy="5.5" rx="7.5" ry="3" />
      <path d="M4.5 5.5v6c0 1.7 3.4 3 7.5 3s7.5-1.3 7.5-3v-6M4.5 11.5v6c0 1.7 3.4 3 7.5 3s7.5-1.3 7.5-3v-6" />
    </>
  ),
  external: (
    <>
      <path d="M14 5h5v5M19 5l-8 8" />
      <path d="M18 13v5.5a1.5 1.5 0 0 1-1.5 1.5h-11A1.5 1.5 0 0 1 4 18.5v-11A1.5 1.5 0 0 1 5.5 6H11" />
    </>
  ),
  file: (
    <>
      <path d="M6 3.5h7l5 5V20a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V4.5a1 1 0 0 1 1-1z" />
      <path d="M13 3.5V9h5M8 13h8M8 16h6" />
    </>
  ),
  filter: (
    <>
      <path d="M4 6h16M7 12h10M10 18h4" />
    </>
  ),
  flask: (
    <>
      <path d="M9 3h6M10 3v5l-5.5 9.2A1.7 1.7 0 0 0 6 20h12a1.7 1.7 0 0 0 1.5-2.8L14 8V3" />
      <path d="M7.5 15h9" />
    </>
  ),
  graph: (
    <>
      <circle cx="6" cy="6" r="2" />
      <circle cx="18" cy="7" r="2" />
      <circle cx="12" cy="18" r="2" />
      <path d="m7.8 7 8.4-.1M7 7.8l3.8 8.4M17 8.8l-3.8 7.4" />
    </>
  ),
  grid: (
    <>
      <rect x="4" y="4" width="6" height="6" rx="1" />
      <rect x="14" y="4" width="6" height="6" rx="1" />
      <rect x="4" y="14" width="6" height="6" rx="1" />
      <rect x="14" y="14" width="6" height="6" rx="1" />
    </>
  ),
  'hard-drive': (
    <>
      <rect x="3.5" y="5" width="17" height="14" rx="2" />
      <path d="M7 9h.01M11 9h6M7 15h.01M11 15h6" />
    </>
  ),
  info: (
    <>
      <circle cx="12" cy="12" r="8.5" />
      <path d="M12 11v5M12 8h.01" />
    </>
  ),
  link: (
    <>
      <path d="M9.5 14.5 14.5 9.5" />
      <path d="M7 17H5.5a3.5 3.5 0 0 1 0-7H9M15 7h1.5a3.5 3.5 0 0 1 0 7H13" />
    </>
  ),
  menu: <path d="M4 7h16M4 12h16M4 17h16" />,
  memory: (
    <>
      <rect x="4" y="4" width="16" height="16" rx="2" />
      <path d="M8 8h8M8 12h5M8 16h8M4 9h2M4 15h2M18 9h2M18 15h2" />
    </>
  ),
  more: (
    <>
      <circle cx="5" cy="12" r="1" fill="currentColor" stroke="none" />
      <circle cx="12" cy="12" r="1" fill="currentColor" stroke="none" />
      <circle cx="19" cy="12" r="1" fill="currentColor" stroke="none" />
    </>
  ),
  plus: <path d="M12 5v14M5 12h14" />,
  play: <path d="m9 6 9 6-9 6z" />,
  refresh: (
    <>
      <path d="M19 8a7.5 7.5 0 0 0-13.2-1.7L4 8.5M4 4v4.5h4.5" />
      <path d="M5 16a7.5 7.5 0 0 0 13.2 1.7l1.8-2.2M20 20v-4.5h-4.5" />
    </>
  ),
  search: (
    <>
      <circle cx="10.8" cy="10.8" r="6.3" />
      <path d="m16 16 4 4" />
    </>
  ),
  server: (
    <>
      <rect x="4" y="4" width="16" height="6" rx="1.5" />
      <rect x="4" y="14" width="16" height="6" rx="1.5" />
      <path d="M8 7h.01M8 17h.01M12 7h5M12 17h5" />
    </>
  ),
  shield: (
    <>
      <path d="M12 3.5 19 6v5.2c0 4.5-2.7 7.7-7 9.3-4.3-1.6-7-4.8-7-9.3V6z" />
      <path d="m8.5 12 2.2 2.2 4.8-5" />
    </>
  ),
  sliders: (
    <>
      <path d="M4 6h10M18 6h2M4 12h2M10 12h10M4 18h10M18 18h2" />
      <circle cx="16" cy="6" r="2" />
      <circle cx="8" cy="12" r="2" />
      <circle cx="16" cy="18" r="2" />
    </>
  ),
  spark: (
    <>
      <path d="m12 3 1.2 5.8L19 10l-5.8 1.2L12 17l-1.2-5.8L5 10l5.8-1.2z" />
      <path d="m19 16 .5 2.5L22 19l-2.5.5L19 22l-.5-2.5L16 19l2.5-.5z" />
    </>
  ),
  target: (
    <>
      <circle cx="12" cy="12" r="8.5" />
      <circle cx="12" cy="12" r="3" />
      <path d="M12 3.5V2M20.5 12H22M12 20.5V22M3.5 12H2" />
    </>
  ),
  trend: (
    <>
      <path d="m4 16 5-5 3 3 7-7" />
      <path d="M14 7h5v5" />
    </>
  ),
  warning: (
    <>
      <path d="m12 4 9 16H3z" />
      <path d="M12 9v5M12 17h.01" />
    </>
  ),
  x: <path d="m7 7 10 10M17 7 7 17" />,
}

export function Icon({ name, size = 18, strokeWidth = 1.8, className }: IconProps) {
  return (
    <svg
      aria-hidden="true"
      className={className}
      fill="none"
      height={size}
      viewBox="0 0 24 24"
      width={size}
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
      strokeWidth={strokeWidth}
    >
      {paths[name]}
    </svg>
  )
}

export function StrataMark({ size = 28 }: { size?: number }) {
  return (
    <svg aria-hidden="true" className="strata-mark" height={size} viewBox="0 0 28 28" width={size}>
      <path d="M4 6.5h20" stroke="var(--color-copper-bright)" strokeWidth="2.4" />
      <path d="M4 12h16" stroke="var(--color-copper)" strokeWidth="2.4" />
      <path d="M4 17.5h11" stroke="var(--color-copper-muted)" strokeWidth="2.4" />
      <path d="M4 23h6" stroke="var(--color-copper-faint)" strokeWidth="2.4" />
    </svg>
  )
}
