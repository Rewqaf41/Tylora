import { useId } from 'react'

/** The Tylora "T": orange and silver slabs over a stem, on a dark tile. */
export function BrandMark({ size = 32 }: { size?: number }) {
  const id = useId()
  const ref = (name: string) => `url(#${id}${name})`
  return <svg className="brand-mark" width={size} height={size} viewBox="0 0 512 512" aria-hidden>
    <defs>
      <linearGradient id={`${id}tile`} x1="0" y1="0" x2="1" y2="1"><stop offset="0" stopColor="#2E343A" /><stop offset="1" stopColor="#14171B" /></linearGradient>
      <linearGradient id={`${id}a`} x1="0" y1="0" x2="1" y2="1"><stop offset="0" stopColor="#FF9A4D" /><stop offset=".55" stopColor="#F26B16" /><stop offset="1" stopColor="#D4520A" /></linearGradient>
      <linearGradient id={`${id}b`} x1="0" y1="0" x2="1" y2="1"><stop offset="0" stopColor="#DCE3E9" /><stop offset="1" stopColor="#9CA9B5" /></linearGradient>
      <linearGradient id={`${id}c`} x1="0" y1="0" x2="0" y2="1"><stop offset="0" stopColor="#AEB8C1" /><stop offset="1" stopColor="#6D7A85" /></linearGradient>
    </defs>
    <rect width="512" height="512" rx="112" fill={ref('tile')} />
    <g transform="translate(4 -32)">
      <path fill={ref('a')} d="M110 115H248Q256 115 256 123V261L105.2 206.7Q92 202 92 188V133Q92 115 110 115Z" />
      <path fill={ref('b')} d="M274 115H394Q412 115 412 133V183Q412 197 399.5 203.3L266 270V123Q266 115 274 115Z" />
      <path fill={ref('c')} d="M173.5 243.7L248.5 270.3Q256 273 256 281V454Q256 462 248.5 459.3L203.6 443.5Q166 430 166 390V249Q166 241 173.5 243.7Z" />
    </g>
  </svg>
}
