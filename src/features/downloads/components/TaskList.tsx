import { useDownloadStore } from '../store/downloadStore';
import { TaskCard } from './TaskCard';
import type { DownloadTask, TaskStatus } from '../types';

const STATUS_ORDER: Record<string, number> = {
  processing: 0,
  queued: 1,
  completed: 2,
  failed: 3,
};

function getStatusKey(status: TaskStatus): string {
  if (typeof status === 'object' && 'failed' in status) return 'failed';
  return status as string;
}

function sortTasks(tasks: DownloadTask[]): DownloadTask[] {
  return [...tasks].sort(
    (a, b) =>
      (STATUS_ORDER[getStatusKey(a.status)] ?? 99) -
      (STATUS_ORDER[getStatusKey(b.status)] ?? 99),
  );
}

export function TaskList() {
  const tasks = useDownloadStore((s) => s.tasks);
  const error = useDownloadStore((s) => s.error);
  const clearError = useDownloadStore((s) => s.clearError);

  const sorted = sortTasks(Object.values(tasks));
  const counts = {
    processing: sorted.filter((t) => t.status === 'processing').length,
    queued: sorted.filter((t) => t.status === 'queued').length,
    completed: sorted.filter((t) => t.status === 'completed').length,
  };

  return (
    <div className="rounded-2xl border border-zinc-800 bg-zinc-900/60 backdrop-blur p-6 space-y-4">
      {/* Header */}
      <div className="flex items-center justify-between">
        <div>
          <h2 className="text-base font-semibold text-white">Download Queue</h2>
          <p className="text-xs text-zinc-500 mt-0.5">
            Live — updates stream from the Tauri backend
          </p>
        </div>

        {/* Summary pills */}
        <div className="flex gap-2 text-xs">
          {counts.processing > 0 && (
            <span className="rounded-full px-2 py-0.5 bg-indigo-500/20 text-indigo-300 ring-1 ring-indigo-500/30">
              {counts.processing} active
            </span>
          )}
          {counts.queued > 0 && (
            <span className="rounded-full px-2 py-0.5 bg-zinc-700 text-zinc-300 ring-1 ring-zinc-600">
              {counts.queued} queued
            </span>
          )}
          {counts.completed > 0 && (
            <span className="rounded-full px-2 py-0.5 bg-emerald-500/20 text-emerald-300 ring-1 ring-emerald-500/30">
              {counts.completed} done
            </span>
          )}
        </div>
      </div>

      {/* Global store error */}
      {error && (
        <div className="flex items-start gap-3 rounded-lg border border-red-800/40 bg-red-900/20 px-4 py-3">
          <span className="text-red-400 text-sm flex-1">{error}</span>
          <button onClick={clearError} className="text-red-400 hover:text-red-300 text-xs shrink-0">
            Dismiss
          </button>
        </div>
      )}

      {/* Task cards */}
      {sorted.length === 0 ? (
        <div className="flex flex-col items-center justify-center py-12 gap-3 text-zinc-600">
          <svg className="w-10 h-10 opacity-40" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={1}>
            <path strokeLinecap="round" strokeLinejoin="round" d="M3 16.5v2.25A2.25 2.25 0 0 0 5.25 21h13.5A2.25 2.25 0 0 0 21 18.75V16.5M16.5 12 12 16.5m0 0L7.5 12m4.5 4.5V3" />
          </svg>
          <p className="text-sm">No downloads queued yet</p>
        </div>
      ) : (
        <div className="space-y-3">
          {sorted.map((task) => (
            <TaskCard key={task.taskId} task={task} />
          ))}
        </div>
      )}
    </div>
  );
}
