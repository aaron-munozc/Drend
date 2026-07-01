import { create } from "zustand";
import { persist, createJSONStorage } from "zustand/middleware";
import { idbStorage } from "./storage.ts";
import { RenderVideoArgs, defaultRenderArgs } from "@/features/render/types/types.ts";

export interface RenderTab {
	id: string;
	title: string;
	jsonFilePath: string | null;
	options: RenderVideoArgs;
}

interface RenderTabStore {
	tabs: RenderTab[];
	activeTabId: string | null;
	addTab: () => void;
	addConfiguredTab: (title: string, jsonFilePath: string, options: RenderVideoArgs) => void; // NEW
	closeTab: (id: string) => void;
	setActiveTab: (id: string) => void;
	updateTab: (id: string, data: Partial<RenderTab>) => void;
	updateTabOptions: (id: string, options: Partial<RenderVideoArgs>) => void;
}

const createNewRenderTab = (): RenderTab => ({
	id: crypto.randomUUID(),
	title: "New Render",
	jsonFilePath: null,
	options: { ...defaultRenderArgs },
});

export const useRenderTabStore = create<RenderTabStore>()(
	persist(
		(set) => ({
			tabs: [createNewRenderTab()],
			activeTabId: null,

			addTab: () => {
				const newTab = createNewRenderTab();
				set((state) => ({
					tabs: [...state.tabs, newTab],
					activeTabId: newTab.id,
				}));
			},

			addConfiguredTab: (title, jsonFilePath, options) => {
				const newTab: RenderTab = {
					id: crypto.randomUUID(),
					title: `Edit: ${title}`,
					jsonFilePath,
					options,
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
						const fallbackTab = createNewRenderTab();
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

			updateTabOptions: (id, newOptions) =>
				set((state) => ({
					tabs: state.tabs.map((tab) =>
						tab.id === id
							? { ...tab, options: { ...tab.options, ...newOptions } }
							: tab
					),
				})),
		}),
		{
			name: "render-tabs-storage",
			storage: createJSONStorage(() => idbStorage),
		}
	)
);