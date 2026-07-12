import { create } from "zustand";
import { persist, createJSONStorage } from "zustand/middleware";
import { Metadata } from "@/features/downloads/types/types.ts";
import {
	defaultRenderArgs,
	RenderVideoArgs,
} from "@/features/render/types/types.ts";
import { idbStorage } from "./storage.ts";

export type TabMode = "select" | "download" | "render";

export interface WorkspaceTab {
	id: string;
	title: string;
	mode: TabMode;

	// Download-specific
	url: string;
	status: "idle" | "loading" | "analyzed" | "error";
	metadata?: Metadata;
	error?: string;
	initialTaskType?: "vodDownload" | "chatDownload";

	// Render-specific
	jsonFilePath: string | null;
	renderOptions: RenderVideoArgs;
	initialOptions?: any; // Generic options storage
}

interface WorkspaceTabStore {
	tabs: WorkspaceTab[];
	activeTabId: string | null;
	addTab: () => void;
	addConfiguredDownloadTab: (
		url: string,
		taskType: "vodDownload" | "chatDownload",
		options: any,
	) => void;
	addConfiguredRenderTab: (
		title: string,
		jsonFilePath: string,
		options: RenderVideoArgs,
	) => void;
	closeTab: (id: string) => void;
	setActiveTab: (id: string) => void;
	updateTab: (id: string, data: Partial<WorkspaceTab>) => void;
	updateRenderOptions: (id: string, options: Partial<RenderVideoArgs>) => void;
}

const createNewTab = (): WorkspaceTab => ({
	id: crypto.randomUUID(),
	title: "New Tab",
	mode: "select",
	url: "",
	status: "idle",
	jsonFilePath: null,
	renderOptions: { ...defaultRenderArgs },
});

export const useWorkspaceStore = create<WorkspaceTabStore>()(
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

			addConfiguredDownloadTab: (url, taskType, options) => {
				const newTab: WorkspaceTab = {
					...createNewTab(),
					mode: "download",
					title: "Edit Configuration",
					url,
					initialTaskType: taskType,
					initialOptions: options,
				};
				set((state) => ({
					tabs: [...state.tabs, newTab],
					activeTabId: newTab.id,
				}));
			},

			addConfiguredRenderTab: (title, jsonFilePath, options) => {
				const newTab: WorkspaceTab = {
					...createNewTab(),
					mode: "render",
					title: `Edit: ${title}`,
					jsonFilePath,
					renderOptions: options,
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
						tab.id === id ? { ...tab, ...data } : tab,
					),
				})),

			updateRenderOptions: (id, newOptions) =>
				set((state) => ({
					tabs: state.tabs.map((tab) =>
						tab.id === id
							? {
									...tab,
									renderOptions: { ...tab.renderOptions, ...newOptions },
								}
							: tab,
					),
				})),
		}),
		{
			name: "workspace-tabs-storage",
			storage: createJSONStorage(() => idbStorage),
		},
	),
);
