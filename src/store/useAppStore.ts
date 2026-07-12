import { create } from "zustand";
import { persist } from "zustand/middleware";

export type AppView = "workspace" | "queue";

interface AppStore {
	activeView: AppView;
	setActiveView: (view: AppView) => void;
	isSidebarCollapsed: boolean;
	toggleSidebar: () => void;
}

export const useAppStore = create<AppStore>()(
	persist(
		(set) => ({
			activeView: "workspace",
			setActiveView: (activeView) => set({ activeView }),
			isSidebarCollapsed: false,
			toggleSidebar: () =>
				set((state) => ({ isSidebarCollapsed: !state.isSidebarCollapsed })),
		}),
		{
			name: "app-view-storage",
		},
	),
);
