import { invoke } from "@tauri-apps/api/core";
import { useDownloadTabStore } from "@/store/useDownloadTabStore.ts";
import {
	FrontendChatOptions,
	FrontendVodOptions,
	Metadata,
} from "@/features/downloads/types/types.ts";

export function useDownloader() {
	const { updateTab } = useDownloadTabStore();

	const analyzeUrl = async (tabId: string, url: string) => {
		updateTab(tabId, { url, status: "loading", error: undefined });
		try {
			// 1. Fetch raw payload (Rust might send snake_case)
			const metadata = await invoke<Metadata>("analyze_stream_url", { url });

			// 3. Extract the nested title safely
			updateTab(tabId, {
				status: "analyzed",
				metadata,
				title: metadata.streamMetadata?.title || "Stream Details",
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
