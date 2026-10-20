import { describe, expect, it } from 'vitest'
import { ease, EASINGS, interpolate } from './easing'

describe('preview easing', () => {
  it('matches the daemon curves at their characteristic points', () => {
    for (const easing of EASINGS) {
      expect(ease(easing, 0)).toBe(0)
      expect(ease(easing, 1)).toBeCloseTo(1, 12)
      expect(ease(easing, -1)).toBe(0)
      expect(ease(easing, 2)).toBeCloseTo(1, 12)
    }
    // Same formulas as tylora-core: ease-out runs ahead, ease-in-out behind.
    expect(ease('ease_out_cubic', 0.25)).toBeCloseTo(1 - 0.75 ** 3, 12)
    expect(ease('ease_in_out_cubic', 0.5)).toBeCloseTo(0.5, 12)
    expect(ease('ease_in_out_cubic', 0.25)).toBeLessThan(0.25)
    expect(ease('ease_out_expo', 0.25)).toBeGreaterThan(ease('ease_out_cubic', 0.25))
  })

  it('interpolates every edge', () => {
    const from = { x: 0, y: 0, width: 100, height: 100 }
    const to = { x: 100, y: -50, width: 300, height: 200 }
    expect(interpolate(from, to, 0.5)).toEqual({ x: 50, y: -25, width: 200, height: 150 })
    expect(interpolate(from, to, 1)).toEqual(to)
  })
})
