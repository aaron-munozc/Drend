import type { TaskStatus } from '../types';

interface Props {
  status: TaskStatus;
}

const LABELS: Record<string, string> = {
  queued: 'Queued',
  processing: 'Processing',
  completed: 'Completed',
};

export function StatusBadge({ status }: Props) {
  const isFailed = typeof status === 'object' && 'failed' in status;
  const label = isFailed ? 'Failed' : LABELS[status as string] ?? status;

  const colorClass = isFailed
    ? 'bg-red-500/20 text-red-300 ring-1 ring-red-500/40'
    : status === 'completed'
    ? 'bg-emerald-500/20 text-emerald-300 ring-1 ring-emerald-500/40'
    : status === 'processing'
    ? 'bg-indigo-500/20 text-indigo-300 ring-1 ring-indigo-500/40'
    : 'bg-zinc-700/60 text-zinc-400 ring-1 ring-zinc-600/40';

  return (
    <span
      className={`inline-flex items-center gap-1.5 rounded-full px-2.5 py-0.5 text-xs font-medium ${colorClass}`}
    >
      {status === 'processing' && (
        <span className="relative flex h-1.5 w-1.5">
          <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-indigo-400 opacity-75" />
          <span className="relative inline-flex rounded-full h-1.5 w-1.5 bg-indigo-400" />
        </span>
      )}
      {label}
    </span>
  );
}
