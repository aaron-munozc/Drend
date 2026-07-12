import { emit } from "@tauri-apps/api/event";
import {
	Activity,
	CheckCircle2,
	Clock,
	DownloadCloud,
	Film,
	Hourglass,
	Layers,
	Loader2,
	MessageSquare,
	Square,
	Trash2,
	XCircle,
	RefreshCw,
	Edit,
} from "lucide-react";
import { useEffect, useState } from "react";
import { AppTask, TaskType, useQueueStore } from "@/store/useQueueStore.ts";
import { useWorkspaceStore } from "@/store/useWorkspaceStore.ts";
import { useAppStore } from "@/store/useAppStore.ts";

export function QueueView() {
	const { tasks, isInitialized, initQueue, clearCompleted } = useQueueStore();
	const [filter, setFilter] = useState<"All" | "Active" | "Completed">("All");

	useEffect(() => {
		if (!isInitialized) {
			initQueue();
		}
	}, [isInitialized, initQueue]);

	const activeCount = tasks.filter(
		(t) =>
			t.status === "processing" ||
			t.status === "merging" ||
			t.status === "queued" ||
			t.status === "pending",
	).length;

	const filteredTasks = tasks.filter((t) => {
		const isFailed = typeof t.status === "object" && "failed" in t.status;
		const isActive =
			t.status === "processing" ||
			t.status === "merging" ||
			t.status === "queued" ||
			t.status === "pending";
		const isDone = t.status === "completed" || isFailed;

		if (filter === "Active") return isActive;
		if (filter === "Completed") return isDone;
		return true;
	});

	return (
		<div className="flex flex-col h-full w-full bg-background overflow-hidden selection:bg-primary/20">
			{/* --- HEADER --- */}
			<div className="shrink-0 p-8 pb-4 border-b border-border bg-card/40 backdrop-blur-md relative z-10 shadow-sm">
				<div className="flex flex-col md:flex-row md:items-center justify-between gap-4">
					<div>
						<h1 className="text-3xl font-bold tracking-tight flex items-center gap-3 text-foreground">
							<div className="p-2 bg-primary/10 rounded-xl">
								<Layers className="w-7 h-7 text-primary" />
							</div>
							Task Pipeline
						</h1>
						<p className="text-muted-foreground mt-2 text-sm">
							Monitor and manage background processing jobs globally.
						</p>
					</div>

					<div className="flex items-center gap-3">
						{activeCount > 0 && (
							<div className="flex items-center gap-2 bg-primary/10 text-primary px-4 py-2 rounded-xl text-sm font-bold shadow-xs animate-in fade-in slide-in-from-right-2 duration-300">
								<Activity className="w-4 h-4 animate-pulse" />
								{activeCount} Active Task{activeCount !== 1 && "s"}
							</div>
						)}
						<button
							onClick={clearCompleted}
							disabled={tasks.length === 0}
							className="flex items-center gap-2 px-4 py-2.5 bg-secondary text-secondary-foreground rounded-xl text-sm font-semibold hover:bg-secondary/80 transition-all disabled:opacity-50 disabled:cursor-not-allowed active:scale-95"
						>
							<Trash2 className="w-4 h-4" />
							Clear History
						</button>
					</div>
				</div>

				{/* --- FILTER TABS --- */}
				<div className="flex items-center gap-6 mt-8">
					{(["All", "Active", "Completed"] as const).map((f) => (
						<button
							key={f}
							onClick={() => setFilter(f)}
							className={`relative pb-3 text-sm font-semibold transition-colors ${
								filter === f
									? "text-primary"
									: "text-muted-foreground hover:text-foreground"
							}`}
						>
							{f}
							<span
								className={`ml-2 text-xs px-2 py-0.5 rounded-full transition-colors ${filter === f ? "bg-primary/10 text-primary" : "bg-muted text-muted-foreground"}`}
							>
								{f === "All"
									? tasks.length
									: f === "Active"
										? activeCount
										: tasks.length - activeCount}
							</span>
							{/* Active Indicator Line */}
							{filter === f && (
								<div className="absolute bottom-0 left-0 right-0 h-0.5 bg-primary rounded-t-full animate-in zoom-in-95 duration-200" />
							)}
						</button>
					))}
				</div>
			</div>

			{/* --- TASK LIST --- */}
			<div className="flex-1 overflow-y-auto p-6 md:p-8 bg-muted/10 relative scroll-smooth">
				<div className="max-w-5xl mx-auto space-y-4">
					{filteredTasks.length === 0 ? (
						<div className="flex flex-col items-center justify-center h-64 text-muted-foreground border-2 border-dashed border-border rounded-2xl bg-card/50 animate-in fade-in zoom-in-95 duration-500">
							<div className="h-16 w-16 rounded-full bg-muted flex items-center justify-center mb-4">
								<Clock className="w-8 h-8 opacity-50" />
							</div>
							<p className="text-lg font-bold text-foreground">
								No {filter !== "All" ? filter.toLowerCase() : ""} tasks found.
							</p>
							<p className="text-sm opacity-80 mt-1">
								Jobs initiated from your unified Workspace will appear
								here[cite: 12].
							</p>
						</div>
					) : (
						<div className="animate-in fade-in slide-in-from-bottom-4 duration-500 space-y-4">
							{filteredTasks
								.map((task) => <TaskCard key={task.taskId} task={task} />)
								.reverse()}
						</div>
					)}
				</div>
			</div>
		</div>
	);
}

// --- TASK CARD COMPONENT ---

function TaskCard({ task }: { task: AppTask }) {
	const { removeTask, retryTask } = useQueueStore();

	const isFailed = typeof task.status === "object" && "failed" in task.status;
	const isCompleted = task.status === "completed";
	const isActive = task.status === "processing" || task.status === "merging";
	const isQueued = task.status === "queued";
	const isPending = task.status === "pending";

	const handleCancel = async () => {
		if (isPending) {
			removeTask(task.taskId);
		} else {
			await emit(`cancel-task-${task.taskId}`);
		}
	};

	// Refactored to utilize the unified Workspace store[cite: 12, 13]
	const handleEditClone = () => {
		if (!task.payload) return;

		if (task.taskType === "chatRender") {
			useWorkspaceStore
				.getState()
				.addConfiguredRenderTab(
					task.title,
					task.payload.jsonFilePath,
					task.payload.options,
				);
		} else {
			useWorkspaceStore
				.getState()
				.addConfiguredDownloadTab(
					task.payload.url,
					task.taskType,
					task.payload.options,
				);
		}
		// Direct the user automatically back to the unified workspace page[cite: 12]
		useAppStore.getState().setActiveView("workspace");
	};

	const getTypeConfig = (type: TaskType) => {
		switch (type) {
			case "vodDownload":
				return {
					icon: <Film className="w-4 h-4" />,
					label: "VOD Download",
					color: "text-blue-500 bg-blue-500/10 border-blue-500/20",
				};
			case "chatDownload":
				return {
					icon: <MessageSquare className="w-4 h-4" />,
					label: "Chat Log",
					color: "text-purple-500 bg-purple-500/10 border-purple-500/20",
				};
			case "chatRender":
				return {
					icon: <DownloadCloud className="w-4 h-4" />,
					label: "Engine Render",
					color: "text-orange-500 bg-orange-500/10 border-orange-500/20",
				};
			default:
				return {
					icon: <Activity className="w-4 h-4" />,
					label: "Process",
					color: "text-gray-500 bg-gray-500/10 border-gray-500/20",
				};
		}
	};

	const typeConfig = getTypeConfig(task.taskType);

	return (
		<div
			className={`relative bg-card border p-5 rounded-2xl shadow-sm flex flex-col gap-4 overflow-hidden transition-all duration-200 group ${
				isFailed
					? "border-destructive/30 bg-destructive/5 hover:border-destructive/50"
					: "border-border hover:border-primary/30 hover:shadow-md"
			}`}
		>
			<div className="flex justify-between items-start gap-4">
				<div className="flex flex-col gap-2 min-w-0">
					<div className="flex items-center gap-3">
						<span
							className={`flex items-center gap-1.5 text-[10px] font-bold uppercase tracking-wider px-2.5 py-1 rounded-lg border shrink-0 ${typeConfig.color}`}
						>
							{typeConfig.icon}
							{typeConfig.label}
						</span>
						<h3
							className="font-semibold text-foreground truncate text-base leading-tight"
							title={task.title}
						>
							{task.title}
						</h3>
					</div>

					<div className="flex items-center gap-2 text-sm font-medium mt-0.5">
						{isActive && (
							<Loader2 className="w-4 h-4 text-primary animate-spin" />
						)}
						{isCompleted && <CheckCircle2 className="w-4 h-4 text-green-500" />}
						{isFailed && <XCircle className="w-4 h-4 text-destructive" />}
						{isQueued && <Clock className="w-4 h-4 text-muted-foreground" />}
						{isPending && (
							<Hourglass className="w-4 h-4 text-muted-foreground animate-pulse" />
						)}

						<span
							className={`${
								isFailed
									? "text-destructive"
									: isCompleted
										? "text-green-500"
										: "text-muted-foreground"
							}`}
						>
							{isFailed
								? (task.status as any).failed
								: task.statusText || task.status}
						</span>
					</div>
				</div>

				<div className="flex gap-2 shrink-0">
					{task.payload && (
						<button
							onClick={handleEditClone}
							className="p-2 bg-muted/50 text-muted-foreground hover:bg-primary/10 hover:text-primary rounded-xl transition-all focus:outline-none focus:ring-2 focus:ring-ring"
							title="Clone settings to workspace"
						>
							<Edit className="w-4 h-4" />
						</button>
					)}

					{isFailed && task.payload && (
						<button
							onClick={() => retryTask(task.taskId)}
							className="group/btn flex items-center gap-2 px-3 py-1.5 bg-green-500/10 text-green-600 hover:bg-green-500 hover:text-white rounded-xl transition-all focus:outline-none focus:ring-2 focus:ring-green-500/50"
							title="Retry Task"
						>
							<RefreshCw className="w-4 h-4 group-hover/btn:rotate-180 transition-transform duration-500" />
							<span className="text-xs font-bold uppercase tracking-wider hidden sm:inline-block">
								Retry
							</span>
						</button>
					)}

					{(isActive || isQueued || isPending) && (
						<button
							onClick={handleCancel}
							className="group/btn flex items-center gap-2 px-3 py-1.5 bg-destructive/10 text-destructive hover:bg-destructive hover:text-white rounded-xl transition-all focus:outline-none focus:ring-2 focus:ring-destructive/50"
							title="Terminate Task"
						>
							<Square className="w-3.5 h-3.5 fill-current group-hover/btn:scale-90 transition-transform" />
							<span className="text-xs font-bold uppercase tracking-wider hidden sm:inline-block">
								Cancel
							</span>
						</button>
					)}
					{(isCompleted || isFailed) && (
						<button
							onClick={() => removeTask(task.taskId)}
							className="p-2 bg-muted/50 text-muted-foreground hover:bg-destructive/10 hover:text-destructive rounded-xl transition-all focus:outline-none focus:ring-2 focus:ring-destructive/50"
							title="Dismiss"
						>
							<Trash2 className="w-4 h-4" />
						</button>
					)}
				</div>
			</div>

			<div className="flex items-center gap-4 w-full mt-1">
				<div className="flex-1 h-2 bg-muted rounded-full overflow-hidden shadow-inner relative">
					<div
						className={`absolute top-0 left-0 bottom-0 transition-all duration-500 ease-out ${
							isFailed
								? "bg-destructive"
								: isCompleted
									? "bg-green-500"
									: isPending
										? "bg-muted-foreground"
										: "bg-primary"
						}`}
						style={{ width: `${Math.max(0, Math.min(100, task.progress))}%` }}
					>
						{isActive && (
							<div className="absolute inset-0 bg-linear-to-r from-transparent via-white/40 to-transparent -translate-x-full animate-[shimmer_1.5s_infinite]" />
						)}
					</div>
				</div>
				<span className="text-xs font-bold text-muted-foreground w-12 text-right tabular-nums tracking-tight">
					{task.progress.toFixed(1)}%
				</span>
			</div>
		</div>
	);
}