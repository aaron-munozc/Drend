import { invoke } from "@tauri-apps/api/core";
import {
	FrontendChatOptions,
	FrontendVodOptions,
	Metadata,
} from "@/features/downloads/types/types.ts";
import { useWorkspaceStore } from "@/store/useWorkspaceStore.ts"; // <-- Added unified store import

export function useDownloader() {
	// Replaced useDownloadTabStore with the unified useWorkspaceStore[cite: 12]
	const { updateTab } = useWorkspaceStore();

	const analyzeUrl = async (tabId: string, url: string) => {
		updateTab(tabId, { url, status: "loading", error: undefined });
		try {
			const metadata = await invoke<Metadata>("analyze_url", { url });

			updateTab(tabId, {
				status: "analyzed",
				metadata,
				title: metadata.normalized.title || "Stream Details",
			});
		} catch (err: any) {
			updateTab(tabId, { status: "error", error: err.toString() });
		}
	};

	const downloadVod = async (url: string, options: FrontendVodOptions) => {
		return await invoke<string>("queue_vod_download", { url, options });
	};

	const downloadChat = async (url: string, options: FrontendChatOptions) => {
		return await invoke<string>("queue_chat_download", { url, options });
	};

	return { analyzeUrl, downloadVod, downloadChat };
}