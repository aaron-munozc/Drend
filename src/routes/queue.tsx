import { createFileRoute } from "@tanstack/react-router";
import { useQueueManager } from "../hooks/useQueueManager";
import { QueueRow } from "../components/QueueRow";
import type { AppTask, TaskType } from "../types/backend";

export const Route = createFileRoute("/queue")({
	component: QueuePage,
});

const TYPE_ORDER: TaskType[] = ["vodDownload", "chatDownload", "chatRender"];

const TYPE_LABELS: Record<TaskType, string> = {
	vodDownload: "VOD Downloads",
	chatDownload: "Chat Downloads",
	chatRender: "Chat Renders",
};

const TYPE_ICONS: Record<TaskType, React.ReactNode> = {
	vodDownload: (
		<svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
			<path d="M8 1a7 7 0 1 0 0 14A7 7 0 0 0 8 1zM6.5 5.5l4 2.5-4 2.5V5.5z" />
		</svg>
	),
	chatDownload: (
		<svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
			<path d="M2 2h12a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1H9l-2 2-2-2H2a1 1 0 0 1-1-1V3a1 1 0 0 1 1-1zm1 2v1h10V4H3zm0 3v1h6V7H3z" />
		</svg>
	),
	chatRender: (
		<svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor">
			<path d="M1 2a1 1 0 0 1 1-1h12a1 1 0 0 1 1 1v12a1 1 0 0 1-1 1H2a1 1 0 0 1-1-1V2zm3 2v8h8V4H4z" />
		</svg>
	),
};

function StatusSummary({
						   tasks,
						   isActive,
					   }: {
	tasks: AppTask[];
	isActive: (t: AppTask) => boolean;
}) {
	const active = tasks.filter(isActive).length;
	const queued = tasks.filter((t) => t.status === "queued").length;
	const done = tasks.filter((t) => t.status === "completed").length;
	const failed = tasks.filter(
		(t) => typeof t.status === "object" && "failed" in t.status,
	).length;

	return (
		<div className="flex items-center gap-3 text-xs">
			{active > 0 && (
				<span className="flex items-center gap-1.5 text-indigo-400 font-medium">
          <span className="w-1.5 h-1.5 rounded-full bg-indigo-400 animate-pulse" />
					{active} active
        </span>
			)}
			{queued > 0 && (
				<span className="flex items-center gap-1.5 text-amber-400 font-medium">
          <span className="w-1.5 h-1.5 rounded-full bg-amber-400" />
					{queued} queued
        </span>
			)}
			{done > 0 && (
				<span className="flex items-center gap-1.5 text-emerald-500 font-medium">
          <span className="w-1.5 h-1.5 rounded-full bg-emerald-500" />
					{done} done
        </span>
			)}
			{failed > 0 && (
				<span className="flex items-center gap-1.5 text-red-400 font-medium">
          <span className="w-1.5 h-1.5 rounded-full bg-red-400" />
					{failed} failed
        </span>
			)}
		</div>
	);
}

function QueuePage() {
	const { tasks, loading, error, cancelTask, moveTask, isActive, isMovable, refetch } =
		useQueueManager();

	const grouped = TYPE_ORDER.reduce<Record<TaskType, AppTask[]>>(
		(acc, type) => {
			acc[type] = tasks.filter((t) => t.taskType === type);
			return acc;
		},
		{} as Record<TaskType, AppTask[]>,
	);

	const hasAnyTasks = tasks.length > 0;

	return (
		<div className="h-full flex flex-col overflow-hidden bg-neutral-950">
			{/* ── Header ──────────────────────────────────────────────────────── */}
			<div className="px-6 py-4 border-b border-neutral-800 flex-shrink-0">
				<div className="flex items-center justify-between">
					<div className="flex items-center gap-3">
						<h1 className="text-sm font-semibold text-neutral-200">
							Operations Queue
						</h1>
						{hasAnyTasks && <StatusSummary tasks={tasks} isActive={isActive} />}
					</div>
					<button
						onClick={refetch}
						className="flex items-center gap-1.5 px-2.5 py-1.5 rounded-md text-neutral-500 hover:text-neutral-300 hover:bg-neutral-800/60 text-xs transition-colors"
						title="Refresh queue"
					>
						<svg
							width="11"
							height="11"
							viewBox="0 0 16 16"
							fill="none"
							stroke="currentColor"
							strokeWidth="2"
							strokeLinecap="round"
							strokeLinejoin="round"
						>
							<path d="M13.5 8a5.5 5.5 0 1 1-1.4-3.7" />
							<polyline points="14 3 14 7 10 7" />
						</svg>
						Refresh
					</button>
				</div>
			</div>

			{/* ── Content ─────────────────────────────────────────────────────── */}
			<div className="flex-1 overflow-y-auto py-5 px-6">
				{loading && (
					<div className="flex items-center gap-3 text-neutral-500 text-sm py-8">
						<span className="w-4 h-4 border-2 border-neutral-700 border-t-indigo-500 rounded-full animate-spin" />
						Loading queue…
					</div>
				)}

				{error && (
					<div className="flex items-start gap-3 bg-red-950/30 border border-red-800/40 rounded-xl px-4 py-3 text-sm text-red-400 mb-4">
						<svg
							className="flex-shrink-0 mt-0.5"
							width="14"
							height="14"
							viewBox="0 0 16 16"
							fill="currentColor"
						>
							<path d="M8 1a7 7 0 1 0 0 14A7 7 0 0 0 8 1zm-.75 4h1.5v5h-1.5V5zm0 6h1.5v1.5h-1.5V11z" />
						</svg>
						<span>
              <strong className="font-semibold">Queue error: </strong>
							{error}
            </span>
					</div>
				)}

				{!loading && !hasAnyTasks && (
					<div className="flex flex-col items-center justify-center py-24 text-neutral-700 gap-4">
						<div className="p-4 rounded-2xl border border-neutral-800/80 bg-neutral-900/30">
							<svg
								width="32"
								height="32"
								viewBox="0 0 40 40"
								fill="none"
								stroke="currentColor"
								strokeWidth="1.5"
							>
								<path d="M8 10h24M8 20h16M8 30h20" strokeLinecap="round" />
							</svg>
						</div>
						<div className="text-center">
							<p className="text-sm font-medium text-neutral-600">Queue is empty</p>
							<p className="text-xs text-neutral-700 mt-1">
								Start an operation from a workspace tab to see it here
							</p>
						</div>
					</div>
				)}

				<div className="space-y-8">
					{TYPE_ORDER.map((type) => {
						const group = grouped[type];
						if (group.length === 0) return null;

						return (
							<section key={type}>
								{/* Section header */}
								<div className="flex items-center gap-2 mb-3">
									<span className="text-neutral-600">{TYPE_ICONS[type]}</span>
									<h2 className="text-[10px] font-bold text-neutral-500 uppercase tracking-[0.15em]">
										{TYPE_LABELS[type]}
									</h2>
									<span className="ml-0.5 text-[10px] text-neutral-700 font-medium">
                    ({group.length})
                  </span>
								</div>

								<div className="space-y-1.5">
									{group.map((task) => {
										const groupMovable = group.filter((t) => isMovable(t));
										const movableIdx = groupMovable.findIndex(
											(t) => t.taskId === task.taskId,
										);
										const canMoveUp = isMovable(task) && movableIdx > 0;
										const canMoveDown =
											isMovable(task) && movableIdx < groupMovable.length - 1;

										return (
											<QueueRow
												key={task.taskId}
												task={task}
												onCancel={cancelTask}
												canMoveUp={canMoveUp}
												canMoveDown={canMoveDown}
												onMoveUp={() => moveTask(task.taskId, "up")}
												onMoveDown={() => moveTask(task.taskId, "down")}
											/>
										);
									})}
								</div>
							</section>
						);
					})}
				</div>
			</div>
		</div>
	);
}