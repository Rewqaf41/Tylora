import type { Easing, Rect } from '../api/types'

/** The daemon's easing curves (crates/tylora-core/src/animation.rs), for the preview. */
export const EASINGS: Easing[] = ['linear', 'ease_out_cubic', 'ease_in_out_cubic', 'ease_out_quart', 'ease_out_expo']

/** Eased progress for linear progress `t` in [0, 1]. */
export function ease(easing: Easing, t: number): number {
  const x = Math.min(1, Math.max(0, t))
  switch (easing) {
    case 'linear': return x
    case 'ease_out_cubic': return 1 - (1 - x) ** 3
    case 'ease_in_out_cubic': return x < 0.5 ? 4 * x * x * x : 1 - (-2 * x + 2) ** 3 / 2
    case 'ease_out_quart': return 1 - (1 - x) ** 4
    case 'ease_out_expo': return x >= 1 ? 1 : 1 - 2 ** (-10 * x)
  }
}

/** Frame between `from` and `to` at eased progress `s`. */
export function interpolate(from: Rect, to: Rect, s: number): Rect {
  const blend = (a: number, b: number) => a + (b - a) * s
  return { x: blend(from.x, to.x), y: blend(from.y, to.y), width: blend(from.width, to.width), height: blend(from.height, to.height) }
}
