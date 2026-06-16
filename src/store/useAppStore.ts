import { create } from "zustand";

export type AppView = "downloads" | "render" | "queue";

interface AppStore {
	activeView: AppView;
	setActiveView: (view: AppView) => void;
	isSidebarCollapsed: boolean;
	toggleSidebar: () => void;
}

export const useAppStore = create<AppStore>((set) => ({
	activeView: "downloads",
	setActiveView: (activeView) => set({ activeView }),
	isSidebarCollapsed: false,
	toggleSidebar: () =>
		set((state) => ({ isSidebarCollapsed: !state.isSidebarCollapsed })),
}));
