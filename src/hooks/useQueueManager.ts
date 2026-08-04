/**
 * useQueueManager.ts
 * Manages the task queue state using a Zustand stores so any component can
 * subscribe without prop-drilling.  State survives hot-reloads and can be
 * read outside of React (e.g. in tray logic).
 */

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect } from "react";
import { create } from "zustand";
import type { AppTask } from "../types/backend";
import { useTauriEvent } from "./useTauriEvent";

// ─────────────────────────────────────────────────────────────────────────────
// Queue stores
// ─────────────────────────────────────────────────────────────────────────────

interface QueueState {
	tasks: AppTask[];
	loading: boolean;
	error: string | null;
	setTasks: (tasks: AppTask[]) => void;
	upsertTask: (task: AppTask) => void;
	setLoading: (v: boolean) => void;
	setError: (e: string | null) => void;
}

export const useQueueStore = create<QueueState>((set) => ({
	tasks: [],
	loading: true,
	error: null,
	setTasks: (tasks) => set({ tasks }),
	upsertTask: (task) =>
		set((s) => {
			const idx = s.tasks.findIndex((t) => t.taskId === task.taskId);
			if (idx === -1) return { tasks: [...s.tasks, task] };
			const next = [...s.tasks];
			next[idx] = task;
			return { tasks: next };
		}),
	setLoading: (loading) => set({ loading }),
	setError: (error) => set({ error }),
}));

// ─────────────────────────────────────────────────────────────────────────────
// Hook (attaches Tauri event listener + initial fetch)
// ─────────────────────────────────────────────────────────────────────────────

export function useQueueManager() {
	const { tasks, loading, error, setTasks, upsertTask, setLoading, setError } =
		useQueueStore();

	const fetchQueue = useCallback(async () => {
		setLoading(true);
		try {
			const result = await invoke<AppTask[]>("get_download_queue");
			setTasks(result);
			setError(null);
		} catch (e) {
			setError(String(e));
		} finally {
			setLoading(false);
		}
	}, [setTasks, setError, setLoading]);

	useEffect(() => {
		fetchQueue();
	}, [fetchQueue]);

	// Live task-progress updates from the Rust backend
	useTauriEvent<AppTask>("task-progress", upsertTask);

	const cancelTask = useCallback(
		async (taskId: string) => {
			try {
				await invoke("cancel_task", { taskId });
				// Optimistically mark as cancelled immediately so UI responds
				upsertTask(
					// We don't know the full task shape here, so just patch status
					// The backend will emit a proper update shortly
					{ ...tasks.find((t) => t.taskId === taskId)!, status: "cancelled" },
				);
			} catch (e) {
				console.error("Failed to cancel task:", e);
			}
		},
		[tasks, upsertTask],
	);

	const isActive = useCallback(
		(task: AppTask) =>
			task.status === "processing" || task.status === "merging",
		[],
	);

	const isMovable = useCallback(
		(task: AppTask) =>
			task.status === "queued" || task.status === "cancelled",
		[],
	);

	const moveTask = useCallback(
		(taskId: string, direction: "up" | "down") => {
			setTasks(
				(() => {
					const prev = useQueueStore.getState().tasks;
					const idx = prev.findIndex((t) => t.taskId === taskId);
					if (idx === -1) return prev;
					const task = prev[idx];
					if (task.status !== "queued" && task.status !== "cancelled")
						return prev;
					const newIdx = direction === "up" ? idx - 1 : idx + 1;
					if (newIdx < 0 || newIdx >= prev.length) return prev;
					const target = prev[newIdx];
					if (target.status !== "queued" && target.status !== "cancelled")
						return prev;
					const next = [...prev];
					next[idx] = target;
					next[newIdx] = task;
					return next;
				})(),
			);
		},
		[setTasks],
	);

	return {
		tasks,
		loading,
		error,
		cancelTask,
		moveTask,
		isActive,
		isMovable,
		refetch: fetchQueue,
	};
}