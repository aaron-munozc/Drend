import type { DownloadTask } from '../types';
import { StatusBadge } from './StatusBadge';

interface Props {
  task: DownloadTask;
}

const PLATFORM_COLORS: Record<string, string> = {
  twitch: 'from-purple-600/20 to-transparent border-purple-500/30',
  kick: 'from-green-600/20 to-transparent border-green-500/30',
};

function getPlatform(title: string): 'twitch' | 'kick' | null {
  if (title.toLowerCase().includes('twitch')) return 'twitch';
  if (title.toLowerCase().includes('kick')) return 'kick';
  return null;
}

export function TaskCard({ task }: Props) {
  const platform = getPlatform(task.title);
  const gradientClass =
    platform && PLATFORM_COLORS[platform]
      ? PLATFORM_COLORS[platform]
      : 'from-zinc-800/20 to-transparent border-zinc-700/30';

  const isFailed =
    typeof task.status === 'object' && 'failed' in task.status;
  const failMessage = isFailed
    ? (task.status as { failed: string }).failed
    : null;

  const progressColor =
    task.status === 'completed'
      ? 'bg-emerald-500'
      : isFailed
      ? 'bg-red-500'
      : 'bg-indigo-500';

  const chunkInfo =
    task.totalEstimatedChunks != null
      ? `${task.currentChunk} / ${task.totalEstimatedChunks} chunks`
      : task.currentChunk > 0
      ? `${task.currentChunk} chunks`
      : null;

  return (
    <div
      className={`rounded-xl border bg-gradient-to-r p-4 ${gradientClass} transition-all duration-300`}
    >
      {/* Header */}
      <div className="flex items-start justify-between gap-3">
        <div className="flex flex-col gap-0.5 min-w-0">
          <p className="text-sm font-semibold text-white truncate">{task.title}</p>
          <p className="text-xs text-zinc-500 font-mono">{task.taskId}</p>
        </div>
        <StatusBadge status={task.status} />
      </div>

      {/* Progress bar */}
      <div className="mt-3 space-y-1.5">
        <div className="relative h-1.5 w-full overflow-hidden rounded-full bg-zinc-800">
          <div
            className={`absolute inset-y-0 left-0 rounded-full transition-all duration-500 ease-out ${progressColor}`}
            style={{ width: `${task.progress}%` }}
          />
          {task.status === 'processing' && (
            <div
              className="absolute inset-y-0 left-0 rounded-full bg-white/20 animate-pulse"
              style={{ width: `${task.progress}%` }}
            />
          )}
        </div>

        <div className="flex items-center justify-between text-xs text-zinc-500">
          <span>{task.progress.toFixed(1)}%</span>
          {chunkInfo && <span>{chunkInfo}</span>}
        </div>
      </div>

      {/* Error message */}
      {failMessage && (
        <p className="mt-2 text-xs text-red-400 bg-red-900/20 rounded-lg px-3 py-2 border border-red-800/40 break-all">
          {failMessage}
        </p>
      )}
    </div>
  );
}
