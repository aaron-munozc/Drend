import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";
import { persist, createJSONStorage } from "zustand/middleware";
import { idbStorage } from "./storage.ts";

export type TaskStatus =
	| "pending"
	| "queued"
	| "processing"
	| "merging"
	| "completed"
	| { failed: string };
export type TaskType = "chatDownload" | "vodDownload" | "chatRender";

export interface AppTask {
	taskId: string;
	taskType: TaskType;
	title: string;
	progress: number;
	status: TaskStatus;
	statusText?: string;
	payload?: any;
}

interface QueueStore {
	tasks: AppTask[];
	isInitialized: boolean;
	maxConcurrent: number;
	initQueue: () => Promise<void>;
	enqueueTask: (taskType: TaskType, title: string, payload: any) => void;
	processQueue: () => void;
	updateTask: (task: AppTask) => void;
	removeTask: (taskId: string) => void;
	clearCompleted: () => void;
	retryTask: (taskId: string) => void;
}

export const useQueueStore = create<QueueStore>()(
	persist(
		(set, get) => ({
			tasks: [],
			isInitialized: false,
			maxConcurrent: 1,

			initQueue: async () => {
				if (get().isInitialized) return;
				try {
					// Fetch currently running tasks from Rust
					const rustTasks = await invoke<AppTask[]>("get_download_queue");

					set((state) => {
						const rustTaskIds = new Set(rustTasks.map((t) => t.taskId));

						// 1. Reconcile frontend tasks: If a task was running but Rust doesn't know about it, the app closed unexpectedly.
						const resolvedPersisted = state.tasks.map((t) => {
							const wasActive =
								t.status === "processing" ||
								t.status === "merging" ||
								t.status === "queued";
							if (wasActive && !rustTaskIds.has(t.taskId)) {
								return {
									...t,
									status: { failed: "Process interrupted (App closed abruptly)" } as TaskStatus,
								};
							}
							return t;
						});

						// 2. Inject payloads back into the Rust tasks (since Rust doesn't store our frontend UI payloads)
						const enrichedRustTasks = rustTasks.map((rt) => {
							const existing = state.tasks.find((t) => t.taskId === rt.taskId);
							return existing ? { ...rt, payload: existing.payload } : rt;
						});

						// 3. Merge avoiding duplicates
						const inactiveFrontendTasks = resolvedPersisted.filter(
							(t) => !rustTaskIds.has(t.taskId)
						);

						return {
							tasks: [...inactiveFrontendTasks, ...enrichedRustTasks],
							isInitialized: true,
						};
					});

					await listen<AppTask>("task-progress", (event) => {
						get().updateTask(event.payload);
					});

					get().processQueue();
				} catch (error) {
					console.error("Failed to fetch initial queue:", error);
				}
			},

			enqueueTask: (taskType, title, payload) => {
				const tempId = crypto.randomUUID();
				const newTask: AppTask = {
					taskId: tempId,
					taskType,
					title,
					progress: 0,
					status: "pending",
					statusText: "Waiting in queue...",
					payload,
				};

				set((state) => ({ tasks: [...state.tasks, newTask] }));
				get().processQueue();
			},

			processQueue: async () => {
				const { tasks, maxConcurrent } = get();
				const activeCount = tasks.filter(
					(t) =>
						t.status === "processing" ||
						t.status === "merging" ||
						t.status === "queued"
				).length;

				if (activeCount >= maxConcurrent) return;

				const nextTask = tasks.find((t) => t.status === "pending");
				if (!nextTask) return;

				set((state) => ({
					tasks: state.tasks.map((t) =>
						t.taskId === nextTask.taskId
							? { ...t, status: "queued", statusText: "Sending to engine..." }
							: t
					),
				}));

				try {
					let endpoint = "";
					if (nextTask.taskType === "vodDownload") endpoint = "queue_vod_download";
					else if (nextTask.taskType === "chatDownload") endpoint = "queue_chat_download";
					else if (nextTask.taskType === "chatRender") endpoint = "queue_chat_render";

					const realTaskId = await invoke<string>(endpoint, nextTask.payload);

					// We no longer drop the payload here so we can re-edit later
					set((state) => ({
						tasks: state.tasks.map((t) =>
							t.taskId === nextTask.taskId
								? { ...t, taskId: realTaskId }
								: t
						),
					}));

					get().processQueue();
				} catch (err: any) {
					set((state) => ({
						tasks: state.tasks.map((t) =>
							t.taskId === nextTask.taskId
								? { ...t, status: { failed: err.toString() } }
								: t
						),
					}));
					get().processQueue();
				}
			},

			updateTask: (incomingTask) => {
				set((state) => {
					const existingIndex = state.tasks.findIndex(
						(t) => t.taskId === incomingTask.taskId
					);
					if (existingIndex >= 0) {
						const newTasks = [...state.tasks];
						// Preserve the payload from the frontend state
						newTasks[existingIndex] = {
							...incomingTask,
							payload: state.tasks[existingIndex].payload,
						};
						return { tasks: newTasks };
					}
					return { tasks: [...state.tasks, incomingTask] };
				});

				const isDone =
					incomingTask.status === "completed" ||
					(typeof incomingTask.status === "object" && "failed" in incomingTask.status);
				if (isDone) {
					get().processQueue();
				}
			},

			removeTask: (taskId) =>
				set((state) => ({
					tasks: state.tasks.filter((t) => t.taskId !== taskId),
				})),

			clearCompleted: () =>
				set((state) => ({
					tasks: state.tasks.filter(
						(t) => t.status !== "completed" && typeof t.status !== "object"
					),
				})),

			retryTask: (taskId) => {
				const task = get().tasks.find((t) => t.taskId === taskId);
				if (!task || !task.payload) return;

				// Remove the failed iteration and queue a fresh one
				get().removeTask(taskId);
				get().enqueueTask(task.taskType, task.title, task.payload);
			},
		}),
		{
			name: "pipeline-queue-storage",
			storage: createJSONStorage(() => idbStorage),
		}
	)
);