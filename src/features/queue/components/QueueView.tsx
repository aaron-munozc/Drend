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
import { useDownloadTabStore } from "@/store/useDownloadTabStore.ts";
import { useRenderTabStore } from "@/store/useRenderTabStore.ts";
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
		<div className="flex flex-col h-full w-full bg-background overflow-hidden">
			{/* HEADER */}
			<div className="shrink-0 p-8 pb-4 border-b border-border bg-card/50">
				<div className="flex flex-col md:flex-row md:items-center justify-between gap-4">
					<div>
						<h1 className="text-3xl font-bold tracking-tight flex items-center gap-3">
							<Layers className="w-8 h-8 text-primary" />
							Task Pipeline
						</h1>
						<p className="text-muted-foreground mt-1">
							Monitor and manage background processing jobs
						</p>
					</div>

					<div className="flex items-center gap-3">
						{activeCount > 0 && (
							<div className="flex items-center gap-2 bg-primary/10 text-primary px-4 py-2 rounded-full text-sm font-bold shadow-xs">
								<Activity className="w-4 h-4 animate-pulse" />
								{activeCount} Active Task{activeCount !== 1 && "s"}
							</div>
						)}
						<button
							onClick={clearCompleted}
							disabled={tasks.length === 0}
							className="flex items-center gap-2 px-4 py-2 bg-secondary text-secondary-foreground rounded-lg text-sm font-semibold hover:bg-secondary/80 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
						>
							<Trash2 className="w-4 h-4" />
							Clear History
						</button>
					</div>
				</div>

				{/* FILTER TABS */}
				<div className="flex items-center gap-6 mt-8">
					{(["All", "Active", "Completed"] as const).map((f) => (
						<button
							key={f}
							onClick={() => setFilter(f)}
							className={`pb-3 text-sm font-semibold transition-colors border-b-2 ${
								filter === f
									? "border-primary text-primary"
									: "border-transparent text-muted-foreground hover:text-foreground hover:border-border"
							}`}
						>
							{f}
							<span className="ml-2 text-xs bg-muted text-muted-foreground px-2 py-0.5 rounded-full">
								{f === "All"
									? tasks.length
									: f === "Active"
										? activeCount
										: tasks.length - activeCount}
							</span>
						</button>
					))}
				</div>
			</div>

			{/* TASK LIST */}
			<div className="flex-1 overflow-y-auto p-8 bg-muted/10">
				<div className="max-w-5xl mx-auto space-y-4">
					{filteredTasks.length === 0 ? (
						<div className="flex flex-col items-center justify-center h-64 text-muted-foreground border-2 border-dashed border-border rounded-xl bg-card/50">
							<Clock className="w-12 h-12 mb-4 opacity-30" />
							<p className="text-lg font-medium">
								No {filter !== "All" ? filter.toLowerCase() : ""} tasks found.
							</p>
							<p className="text-sm opacity-70">
								Jobs added from the Downloads or Render tabs will appear here.
							</p>
						</div>
					) : (
						filteredTasks
							.map((task) => <TaskCard key={task.taskId} task={task} />)
							.reverse()
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

	const handleEditClone = () => {
		if (!task.payload) return;

		if (task.taskType === "chatRender") {
			useRenderTabStore.getState().addConfiguredTab(
				task.title,
				task.payload.jsonFilePath,
				task.payload.options
			);
			useAppStore.getState().setActiveView("render");
		} else {
			useDownloadTabStore.getState().addConfiguredTab(
				task.payload.url,
				task.taskType,
				task.payload.options
			);
			useAppStore.getState().setActiveView("downloads");
		}
	};

	const getTypeConfig = (type: TaskType) => {
		switch (type) {
			case "vodDownload":
				return {
					icon: <Film className="w-4 h-4" />,
					label: "VOD Download",
					color: "text-blue-500 bg-blue-500/10",
				};
			case "chatDownload":
				return {
					icon: <MessageSquare className="w-4 h-4" />,
					label: "Chat Log",
					color: "text-purple-500 bg-purple-500/10",
				};
			case "chatRender":
				return {
					icon: <DownloadCloud className="w-4 h-4" />,
					label: "Engine Render",
					color: "text-orange-500 bg-orange-500/10",
				};
			default:
				return {
					icon: <Activity className="w-4 h-4" />,
					label: "Process",
					color: "text-gray-500 bg-gray-500/10",
				};
		}
	};

	const typeConfig = getTypeConfig(task.taskType);

	return (
		<div
			className={`relative bg-card border p-5 rounded-xl shadow-sm flex flex-col gap-4 overflow-hidden transition-all ${isFailed ? "border-destructive/50 bg-destructive/5" : "border-border hover:border-primary/30"}`}
		>
			<div className="flex justify-between items-start gap-4">
				<div className="flex flex-col gap-1.5 min-w-0">
					<div className="flex items-center gap-3">
						<span
							className={`flex items-center gap-1.5 text-xs font-bold uppercase tracking-wider px-2.5 py-1 rounded-md shrink-0 ${typeConfig.color}`}
						>
							{typeConfig.icon}
							{typeConfig.label}
						</span>
						<h3
							className="font-semibold text-foreground truncate text-base"
							title={task.title}
						>
							{task.title}
						</h3>
					</div>

					<div className="flex items-center gap-2 text-sm font-medium mt-1">
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
							className={`${isFailed ? "text-destructive" : isCompleted ? "text-green-500" : "text-muted-foreground"}`}
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
							className="p-2 bg-muted text-muted-foreground hover:bg-primary/20 hover:text-primary rounded-md transition-colors"
							title="Clone settings to a new tab"
						>
							<Edit className="w-4 h-4" />
						</button>
					)}

					{isFailed && task.payload && (
						<button
							onClick={() => retryTask(task.taskId)}
							className="group flex items-center gap-2 px-3 py-1.5 bg-green-500/10 text-green-600 hover:bg-green-500 hover:text-white rounded-md transition-colors"
							title="Retry Task"
						>
							<RefreshCw className="w-4 h-4 group-hover:rotate-180 transition-transform duration-500" />
							<span className="text-xs font-bold uppercase tracking-wider">
								Retry
							</span>
						</button>
					)}

					{(isActive || isQueued || isPending) && (
						<button
							onClick={handleCancel}
							className="group flex items-center gap-2 px-3 py-1.5 bg-destructive/10 text-destructive hover:bg-destructive hover:text-white rounded-md transition-colors"
							title="Terminate Task"
						>
							<Square className="w-4 h-4 fill-current group-hover:scale-90 transition-transform" />
							<span className="text-xs font-bold uppercase tracking-wider">
								Cancel
							</span>
						</button>
					)}
					{(isCompleted || isFailed) && (
						<button
							onClick={() => removeTask(task.taskId)}
							className="p-2 bg-muted text-muted-foreground hover:bg-muted-foreground/20 hover:text-foreground rounded-md transition-colors"
							title="Dismiss"
						>
							<Trash2 className="w-4 h-4" />
						</button>
					)}
				</div>
			</div>

			<div className="flex items-center gap-4 w-full">
				<div className="flex-1 h-2.5 bg-muted rounded-full overflow-hidden shadow-inner">
					<div
						className={`h-full transition-all duration-500 ease-out relative ${
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
							<div className="absolute inset-0 bg-linear-to-r from-transparent via-white/30 to-transparent -translate-x-full animate-[shimmer_1.5s_infinite]" />
						)}
					</div>
				</div>
				<span className="text-xs font-bold text-muted-foreground w-12 text-right tabular-nums">
					{task.progress.toFixed(1)}%
				</span>
			</div>
		</div>
	);
}