import { invoke } from "@tauri-apps/api/core";
import { useTabStore } from "@/store/useTabStore.ts";
import {
	FrontendChatOptions,
	FrontendVodOptions,
	Metadata,
} from "@/features/downloads/types/types.ts";

export function useDownloader() {
	const { updateTab } = useTabStore();

	const analyzeUrl = async (tabId: string, url: string) => {
		updateTab(tabId, { url, status: "loading", error: undefined });
		try {
			// 1. Fetch raw payload (Rust might send snake_case)
			const rawData = await invoke<any>("analyze_stream_url", { url });

			// 2. Normalize to your frontend camelCase interface
			const metadata: Metadata = {
				streamMetadata: rawData.streamMetadata || rawData.stream_metadata,
				qualities: rawData.qualities || [],
			};

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
