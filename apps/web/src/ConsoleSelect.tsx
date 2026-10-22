import type { SelectHTMLAttributes } from 'react';
import { useText } from './i18n';

type Value = string | number;
type Props<T extends Value> = Omit<SelectHTMLAttributes<HTMLSelectElement>, 'value' | 'defaultValue' | 'onChange' | 'children'> & {
  value?: T;
  options: { value: T; label: string; disabled?: boolean }[];
} & ({ allowClear: true; onChange?: (value: T | undefined) => void } | { allowClear?: false; onChange?: (value: T) => void });

/** Native popup behavior with the original option value types preserved for forms. */
export function ConsoleSelect<T extends Value>(props: Props<T>) {
  const t = useText();
  const { options, value, allowClear, onChange: _onChange, className, ...attributes } = props;
  return <select {...attributes} className={`console-select${className ? ` ${className}` : ''}`} value={value ?? ''} onChange={event => {
    const selected = options.find(option => String(option.value) === event.target.value);
    if (selected) props.onChange?.(selected.value);
    else if (props.allowClear) props.onChange?.(undefined);
  }}>
    {(allowClear || value === undefined) && <option value="" disabled={!allowClear}>{allowClear ? t('all') : ''}</option>}
    {options.map(option => <option key={String(option.value)} value={option.value} disabled={option.disabled}>{option.label}</option>)}
  </select>;
}

/** Ant Design pagination exposes a public hook for its size selection control. */
export function PageSizeSelect({ value, onChange, disabled, className }: {
  value: number; onChange: (value: number) => void; disabled: boolean; className: string;
}) {
  const t = useText(); const sizes = [...new Set([10, 20, 24, 50, 100, value])].sort((a, b) => a - b);
  return <ConsoleSelect className={className} aria-label={t('auditPageSize')} value={value} onChange={onChange} disabled={disabled} options={sizes.map(size => ({ value: size, label: `${size} ${t('perPage')}` }))}/>;
}

export const paginationComponents = { sizeChanger: PageSizeSelect };
