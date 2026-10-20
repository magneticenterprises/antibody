import { useEffect, useId, useRef, useState, type FormEvent, type ReactNode } from 'react';

interface Props {
  /** Trigger button text. */
  label: string;
  /** Heading of the confirmation panel. */
  title: string;
  /** What will happen; shown before the user confirms. */
  description: ReactNode;
  confirmLabel: string;
  tone?: 'danger' | 'default';
  pending?: boolean;
  disabled?: boolean;
  /** Optional extra fields (e.g. a reason). */
  children?: ReactNode;
  /** Return `false` to keep the panel open (e.g. when the call failed). */
  onConfirm: () => Promise<boolean | void> | boolean | void;
}

/**
 * Two-step confirmation rendered inline (no modal): the trigger reveals a
 * panel that states the consequence; Escape or Cancel returns focus.
 */
export function ConfirmAction({
  label,
  title,
  description,
  confirmLabel,
  tone = 'default',
  pending = false,
  disabled = false,
  children,
  onConfirm,
}: Props) {
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLFormElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  const titleId = useId();
  const descId = useId();

  useEffect(() => {
    if (!open) return;
    // Focus the first field, otherwise the safe choice (Cancel), never the destructive button.
    const field = panelRef.current?.querySelector<HTMLElement>('input, textarea, select');
    (field ?? cancelRef.current)?.focus();
  }, [open]);

  const close = () => {
    setOpen(false);
    window.setTimeout(() => triggerRef.current?.focus(), 0);
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const result = await onConfirm();
    if (result !== false) close();
  };

  if (!open) {
    return (
      <button
        ref={triggerRef}
        type="button"
        className={`button${tone === 'danger' ? ' button--danger-quiet' : ''}`}
        onClick={() => setOpen(true)}
        disabled={disabled}
        aria-expanded={false}
      >
        {label}
      </button>
    );
  }

  return (
    <form
      ref={panelRef}
      className={`confirm${tone === 'danger' ? ' confirm--danger' : ''}`}
      role="alertdialog"
      aria-modal="false"
      aria-labelledby={titleId}
      aria-describedby={descId}
      onSubmit={submit}
      onKeyDown={(e) => {
        if (e.key === 'Escape') {
          e.stopPropagation();
          close();
        }
      }}
    >
      <p id={titleId} className="confirm__title">
        {title}
      </p>
      <div id={descId} className="confirm__desc">
        {description}
      </div>
      {children}
      <div className="button-row">
        <button type="submit" className={`button ${tone === 'danger' ? 'button--danger' : 'button--primary'}`} disabled={pending}>
          {pending ? 'Working…' : confirmLabel}
        </button>
        <button ref={cancelRef} type="button" className="button button--quiet" onClick={close} disabled={pending}>
          Cancel
        </button>
      </div>
    </form>
  );
}
