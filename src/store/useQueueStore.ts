import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { create } from "zustand";

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
	payload?: any; // Temporarily holds the form args before sending to Rust
}

interface QueueStore {
	tasks: AppTask[];
	isInitialized: boolean;
	maxConcurrent: number; // Controls how many tasks run at once
	initQueue: () => Promise<void>;
	enqueueTask: (taskType: TaskType, title: string, payload: any) => void;
	processQueue: () => void;
	updateTask: (task: AppTask) => void;
	removeTask: (taskId: string) => void;
	clearCompleted: () => void;
}

export const useQueueStore = create<QueueStore>((set, get) => ({
	tasks: [],
	isInitialized: false,
	maxConcurrent: 1, // Will wait for current task to finish before starting the next

	initQueue: async () => {
		if (get().isInitialized) return;
		try {
			// 1. Fetch anything already running in Rust
			const rustTasks = await invoke<AppTask[]>("get_download_queue");

			set((state) => {
				// Keep frontend "pending" tasks, merge with real Rust tasks
				const pending = state.tasks.filter((t) => t.status === "pending");
				return { tasks: [...rustTasks, ...pending], isInitialized: true };
			});

			// 2. Set up the GLOBAL listener so it never unmounts
			await listen<AppTask>("task-progress", (event) => {
				get().updateTask(event.payload);
			});

			// 3. Kickstart the queue in case we have pending items
			get().processQueue();
		} catch (error) {
			console.error("Failed to fetch initial queue:", error);
		}
	},

	enqueueTask: (taskType, title, payload) => {
		const tempId = crypto.randomUUID(); // Temporary frontend ID
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

		// Count how many tasks are actively touching the Rust engine
		const activeCount = tasks.filter(
			(t) =>
				t.status === "processing" ||
				t.status === "merging" ||
				t.status === "queued",
		).length;

		if (activeCount >= maxConcurrent) return; // Queue is busy, wait.

		// Find the first task waiting to be processed
		const nextTask = tasks.find((t) => t.status === "pending");
		if (!nextTask) return; // Nothing left to do

		// Optimistically mark it so the loop doesn't grab it twice
		set((state) => ({
			tasks: state.tasks.map((t) =>
				t.taskId === nextTask.taskId
					? { ...t, status: "queued", statusText: "Sending to engine..." }
					: t,
			),
		}));

		try {
			// Map the type to the exact Tauri command
			let endpoint = "";
			if (nextTask.taskType === "vodDownload") endpoint = "queue_vod_download";
			else if (nextTask.taskType === "chatDownload")
				endpoint = "queue_chat_download";
			else if (nextTask.taskType === "chatRender")
				endpoint = "queue_chat_render";

			// Dispatch to Rust and get the REAL task ID
			const realTaskId = await invoke<string>(endpoint, nextTask.payload);

			// Swap the temp ID for the real ID and drop the payload
			set((state) => ({
				tasks: state.tasks.map((t) =>
					t.taskId === nextTask.taskId
						? { ...t, taskId: realTaskId, payload: undefined }
						: t,
				),
			}));

			// Fire again in case maxConcurrent was increased
			get().processQueue();
		} catch (err: any) {
			// If it fails immediately, mark it and trigger the next one
			set((state) => ({
				tasks: state.tasks.map((t) =>
					t.taskId === nextTask.taskId
						? { ...t, status: { failed: err.toString() } }
						: t,
				),
			}));
			get().processQueue();
		}
	},

	updateTask: (incomingTask) => {
		set((state) => {
			const existingIndex = state.tasks.findIndex(
				(t) => t.taskId === incomingTask.taskId,
			);
			if (existingIndex >= 0) {
				const newTasks = [...state.tasks];
				newTasks[existingIndex] = incomingTask;
				return { tasks: newTasks };
			}
			return { tasks: [...state.tasks, incomingTask] };
		});

		// CRITICAL: If a task just finished, process the next one!
		const isDone =
			incomingTask.status === "completed" ||
			(typeof incomingTask.status === "object" &&
				"failed" in incomingTask.status);
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
				(t) => t.status !== "completed" && typeof t.status !== "object",
			),
		})),
}));
