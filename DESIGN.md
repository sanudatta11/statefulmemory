# Design System

<!-- impeccable:design-schema 1 -->

## World

**Memory strata / sediment core** — local observations settle as layered mineral bands on a deep slate ground. Copper marks “live / local” instrument cues. Seed `6f2e0391`, assigned grounded candidate 5 (strata), raised with nixie-precision status, dive-depth scroll bands, and teach-annotation on the install core sample.

## Palette

| Role | Dark | Light |
|---|---|---|
| Ground | `#0f1714` mineral slate | `#f4f7f5` cool stone |
| Foreground | `#f0f4f1` | `#121c18` |
| Muted | `#a8b5ae` | `#3d4a44` |
| Accent | `#c8783a` copper | `#9a5520` copper deep |
| Accent bright | `#e8a56a` | `#8a4f22` |
| Hairline | `#2a3832` | `#d5ddd8` |

Strategy: **Restrained** neutrals + one copper accent (Read/Operate docs chrome); Persuade splash uses committed copper on primary CTA and strata atmosphere.

## Typography

- **Display:** Bricolage Grotesque Variable — wordmark, headings
- **Body:** Source Sans 3 Variable — UI and prose
- **Mono:** JetBrains Mono — commands, core sample, status labels

## Components

- Starlight chrome retained; themed via `--sl-*` tokens in `website/src/styles/custom.css`
- Custom `Hero` override: brand-first splash, install “core sample”, local pulse — no card grid, no eyebrow above the wordmark
- Favicon: four horizontal strata bars (slate → copper)

## Motion

- No looping pulse or entrance animations; calm static strata atmosphere only.

## Surfaces

- `/` — Persuade splash (strata field)
- `/docs/**` — Read mode sharing tokens, copper current-page accent
