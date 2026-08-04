import { useNavigate } from "@tanstack/react-router";
import { TabSnapshot, useWorkspaceStore } from "@/stores/useWorkspaceStore.ts";
import type { AppTask, TaskStatus, TaskType } from "../types/backend";
import { ProgressBar } from "./ui/ProgressBar";

interface QueueRowProps {
	task: AppTask;
	onCancel: (taskId: string) => void;
	canMoveUp: boolean;
	canMoveDown: boolean;
	onMoveUp: () => void;
	onMoveDown: () => void;
}

function statusLabel(status: TaskStatus): string {
	if (typeof status === "string") return status;
	return "Failed";
}

function statusColor(status: TaskStatus): string {
	if (status === "completed") return "text-emerald-400";
	if (status === "cancelled") return "text-neutral-500";
	if (status === "processing" || status === "merging") return "text-indigo-400";
	if (status === "queued") return "text-amber-400";
	return "text-red-400";
}

function typeLabel(type: TaskType): string {
	const map: Record<TaskType, string> = {
		vodDownload: "VOD",
		chatDownload: "Chat",
		chatRender: "Render",
	};
	return map[type];
}

function typeBadgeColor(type: TaskType): string {
	const map: Record<TaskType, string> = {
		vodDownload: "bg-indigo-900/50 text-indigo-300 border-indigo-700/50",
		chatDownload: "bg-amber-900/50 text-amber-300 border-amber-700/50",
		chatRender: "bg-violet-900/50 text-violet-300 border-violet-700/50",
	};
	return map[type];
}

const isActive = (status: TaskStatus) =>
	status === "processing" || status === "merging";

const isFinished = (status: TaskStatus) =>
	status === "completed" ||
	status === "cancelled" ||
	typeof status === "object";

export function QueueRow({
							 task,
							 onCancel,
							 canMoveUp,
							 canMoveDown,
							 onMoveUp,
							 onMoveDown,
						 }: QueueRowProps) {
	const navigate = useNavigate();

	const handleReturn = () => {
		const store = useWorkspaceStore.getState();
		const snapshot: TabSnapshot | undefined = store.getSnapshotByTaskId(task.taskId);
		if (!snapshot) return;

		const existingTab = store.tabs.find((t) => t.id === snapshot.tabId);
		if (existingTab) {
			store.setActiveTab(existingTab.id);
			navigate({ to: "/" });
			return;
		}

		// Reconstruct tab synchronously
		store.addTab({
			url: snapshot.url ?? "",
			jsonFilePath: snapshot.jsonFilePath ?? "",
			vodOptions: snapshot.vodOptions ?? {},
			chatOptions: snapshot.chatOptions ?? {},
			renderOptions: snapshot.renderOptions ?? {},
			downloadMode: snapshot.downloadMode ?? "vod",
			metadata: snapshot.metadata ?? null,
			label: snapshot.metadata?.title ?? "Restored Workspace",
		});

		navigate({ to: "/" });
	};

	const active = isActive(task.status);
	const finished = isFinished(task.status);
	const failed = typeof task.status === "object";
	const errorMessage = failed ? (task.status as { failed: string }).failed : null;

	return (
		<div
			className={`bg-neutral-900/60 border rounded-xl p-4 space-y-3 transition-colors ${
				failed
					? "border-red-800/40"
					: active
						? "border-indigo-700/40"
						: "border-neutral-800"
			}`}
		>
			<div className="flex items-start gap-3">
				{/* Reorder controls */}
				<div className="flex flex-col gap-0.5 pt-0.5 flex-shrink-0">
					<button
						type="button"
						onClick={onMoveUp}
						disabled={!canMoveUp}
						className="w-5 h-5 flex items-center justify-center text-neutral-600 hover:text-neutral-300 disabled:opacity-20 disabled:cursor-not-allowed transition-colors"
						aria-label="Move up"
					>
						<svg width="10" height="10" viewBox="0 0 10 10" fill="currentColor">
							<path d="M5 2L9 7H1L5 2Z" />
						</svg>
					</button>
					<button
						type="button"
						onClick={onMoveDown}
						disabled={!canMoveDown}
						className="w-5 h-5 flex items-center justify-center text-neutral-600 hover:text-neutral-300 disabled:opacity-20 disabled:cursor-not-allowed transition-colors"
						aria-label="Move down"
					>
						<svg width="10" height="10" viewBox="0 0 10 10" fill="currentColor">
							<path d="M5 8L1 3H9L5 8Z" />
						</svg>
					</button>
				</div>

				{/* Main content */}
				<div className="flex-1 min-w-0">
					<div className="flex items-center gap-2 mb-1">
						<span
							className={`text-xs px-2 py-0.5 rounded-md border font-medium ${typeBadgeColor(
								task.taskType
							)}`}
						>
							{typeLabel(task.taskType)}
						</span>
						<span
							className={`text-xs font-medium capitalize ${statusColor(
								task.status
							)}`}
						>
							{statusLabel(task.status)}
						</span>
					</div>

					<p className="text-sm text-neutral-200 font-medium truncate">
						{task.title}
					</p>

					{task.statusText && (
						<p className="text-xs text-neutral-500 mt-0.5 truncate">
							{task.statusText}
						</p>
					)}

					{errorMessage && (
						<p className="text-xs text-red-400 mt-1">Error: {errorMessage}</p>
					)}
				</div>

				{/* Actions */}
				<div className="flex items-center gap-2 flex-shrink-0">
					<span className="text-xs font-mono text-neutral-500 tabular-nums">
						{task.progress.toFixed(1)}%
					</span>
					{active && (
						<button
							type="button"
							onClick={() => onCancel(task.taskId)}
							className="px-3 py-1 text-xs bg-red-950/50 hover:bg-red-900/60 text-red-400 hover:text-red-300 border border-red-800/40 rounded-lg transition-colors"
						>
							Cancel
						</button>
					)}
				</div>
			</div>

			{/* Progress */}
			{active && (
				<ProgressBar
					value={task.progress}
					color={
						task.taskType === "chatRender"
							? "linear-gradient(90deg, #7c3aed, #a78bfa)"
							: "linear-gradient(90deg, #4f46e5, #818cf8)"
					}
				/>
			)}

			{task.status === "completed" && (
				<ProgressBar
					value={100}
					color="linear-gradient(90deg, #059669, #34d399)"
				/>
			)}

			{/* Return link for ALL finished tasks */}
			{finished && (
				<button
					type="button"
					onClick={handleReturn}
					className="text-xs text-indigo-400 hover:text-indigo-300 transition-colors underline underline-offset-2"
				>
					← Return to Origin Tab Workspace
				</button>
			)}
		</div>
	);
}