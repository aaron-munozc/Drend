import { create } from "zustand";
import { persist, createJSONStorage } from "zustand/middleware";
import { idbStorage } from "./storage.ts";
import { Metadata } from "@/features/downloads/types/types.ts";

export interface DownloadTab {
	id: string;
	title: string;
	url: string;
	status: "idle" | "loading" | "analyzed" | "error";
	metadata?: Metadata;
	error?: string;
	initialTaskType?: "vodDownload" | "chatDownload"; // NEW
	initialOptions?: any;
}

interface DownloadTabStore {
	tabs: DownloadTab[];
	activeTabId: string | null;
	addTab: () => void;
	addConfiguredTab: (url: string, taskType: "vodDownload" | "chatDownload", options: any) => void;
	closeTab: (id: string) => void;
	setActiveTab: (id: string) => void;
	updateTab: (id: string, data: Partial<DownloadTab>) => void;
}

const createNewTab = (): DownloadTab => ({
	id: crypto.randomUUID(),
	title: "New Download",
	url: "",
	status: "idle",
});

export const useDownloadTabStore = create<DownloadTabStore>()(
	persist(
		(set) => ({
			tabs: [createNewTab()],
			activeTabId: null,

			addTab: () => {
				const newTab = createNewTab();
				set((state) => ({
					tabs: [...state.tabs, newTab],
					activeTabId: newTab.id,
				}));
			},

			addConfiguredTab: (url, taskType, options) => {
				const newTab: DownloadTab = {
					id: crypto.randomUUID(),
					title: "Edit Configuration",
					url,
					status: "idle",
					initialTaskType: taskType,
					initialOptions: options,
				};
				set((state) => ({
					tabs: [...state.tabs, newTab],
					activeTabId: newTab.id,
				}));
			},

			closeTab: (id) =>
				set((state) => {
					const newTabs = state.tabs.filter((t) => t.id !== id);
					if (newTabs.length === 0) {
						const fallbackTab = createNewTab();
						return { tabs: [fallbackTab], activeTabId: fallbackTab.id };
					}
					const newActiveId =
						state.activeTabId === id
							? newTabs[newTabs.length - 1].id
							: state.activeTabId;
					return { tabs: newTabs, activeTabId: newActiveId };
				}),

			setActiveTab: (id) => set({ activeTabId: id }),

			updateTab: (id, data) =>
				set((state) => ({
					tabs: state.tabs.map((tab) =>
						tab.id === id ? { ...tab, ...data } : tab
					),
				})),
		}),
		{
			name: "download-tabs-storage",
			storage: createJSONStorage(() => idbStorage),
		}
	)
);