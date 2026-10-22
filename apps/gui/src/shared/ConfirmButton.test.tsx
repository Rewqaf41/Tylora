import { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ConfirmButton } from './ui'

describe('ConfirmButton', () => {
  let host: HTMLDivElement
  let root: Root
  const button = (label: string) => [...host.querySelectorAll('button')].find((node) => node.textContent?.trim() === label)
  beforeEach(() => {
    Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true })
    host = document.createElement('div')
    document.body.append(host)
    root = createRoot(host)
  })
  afterEach(() => { act(() => root.unmount()); host.remove() })

  it('asks inline and acts only after Confirm, without window.confirm', () => {
    const onConfirm = vi.fn()
    const nativeConfirm = vi.spyOn(window, 'confirm').mockReturnValue(false)
    act(() => root.render(<ConfirmButton prompt="Stop Tylora?" onConfirm={onConfirm}>Stop</ConfirmButton>))
    act(() => button('Stop')?.click())
    expect(host.textContent).toContain('Stop Tylora?')
    expect(onConfirm).not.toHaveBeenCalled()
    act(() => button('Confirm')?.click())
    expect(onConfirm).toHaveBeenCalledTimes(1)
    expect(nativeConfirm).not.toHaveBeenCalled()
    expect(button('Stop')).toBeDefined()
  })

  it('cancels without acting', () => {
    const onConfirm = vi.fn()
    act(() => root.render(<ConfirmButton prompt="Restart?" onConfirm={onConfirm}>Restart</ConfirmButton>))
    act(() => button('Restart')?.click())
    act(() => button('Cancel')?.click())
    expect(onConfirm).not.toHaveBeenCalled()
    expect(button('Restart')).toBeDefined()
  })
})
