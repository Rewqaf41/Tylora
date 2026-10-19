import type { ButtonHTMLAttributes, PropsWithChildren, ReactNode } from 'react'
import { AlertCircle, Inbox, LoaderCircle, ShieldAlert } from 'lucide-react'
import { useT } from '../i18n'

export function Button({ children, variant = 'secondary', ...props }: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: 'primary' | 'secondary' | 'danger' | 'ghost' }) {
  return <button className={`button button-${variant}`} {...props}>{children}</button>
}
export function Card({ children, className = '' }: PropsWithChildren<{ className?: string }>) {
  return <section className={`card ${className}`}>{children}</section>
}
export function SectionHeader({ title, description, aside }: { title: string; description?: string; aside?: ReactNode }) {
  return <div className="section-header"><div><h2>{title}</h2>{description && <p>{description}</p>}</div>{aside}</div>
}
export function Badge({ children, tone = 'neutral' }: PropsWithChildren<{ tone?: 'neutral' | 'good' | 'warn' | 'bad' }>) {
  return <span className={`badge badge-${tone}`}>{children}</span>
}
export function Notice({ children, tone = 'info' }: PropsWithChildren<{ tone?: 'info' | 'warn' | 'bad' }>) {
  const Icon = tone === 'warn' ? ShieldAlert : AlertCircle
  return <div className={`notice notice-${tone}`}><Icon size={16} /> <span>{children}</span></div>
}
export function Empty({ title, children }: PropsWithChildren<{ title: string }>) {
  return <div className="empty"><Inbox size={25} /><strong>{title}</strong><span>{children}</span></div>
}
export function Loading() { const t = useT(); return <div className="center-state"><LoaderCircle size={22} className="spin" /><span>{t('common.loading')}</span></div> }
export function ErrorState({ message, retry }: { message: string; retry?: () => void }) {
  const t = useT()
  return <div className="center-state"><AlertCircle size={24} /><strong>{t('common.loadFailed')}</strong><span>{message}</span>{retry && <Button onClick={retry}>{t('common.tryAgain')}</Button>}</div>
}
export function Metric({ label, value, detail }: { label: string; value: ReactNode; detail?: string }) {
  return <Card className="metric"><span className="metric-label">{label}</span><strong>{value}</strong>{detail && <small>{detail}</small>}</Card>
}
export function ConfirmButton({ children, prompt, onConfirm, disabled, variant = 'danger' }: PropsWithChildren<{ prompt: string; onConfirm: () => void; disabled?: boolean; variant?: 'danger' | 'secondary' }>) {
  return <Button variant={variant} disabled={disabled} onClick={() => { if (window.confirm(prompt)) onConfirm() }}>{children}</Button>
}
