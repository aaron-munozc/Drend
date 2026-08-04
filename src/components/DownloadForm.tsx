import { invoke } from "@tauri-apps/api/core";
import { useNavigate } from "@tanstack/react-router";
import type {
	Metadata,
} from "../types/backend";
import {DownloadMode, TabState, useWorkspace} from "@/stores/useWorkspaceStore.ts";

interface DownloadFormProps {
	tab: TabState;
	onUpdate: (patch: Partial<TabState>) => void;
}

function formatDuration(seconds?: number): string {
	if (!seconds) return "Unknown";
	const h = Math.floor(seconds / 3600);
	const m = Math.floor((seconds % 3600) / 60);
	const s = seconds % 60;
	if (h > 0)
		return `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
	return `${m}:${String(s).padStart(2, "0")}`;
}

export function DownloadForm({ tab, onUpdate }: DownloadFormProps) {
	const { registerTaskSnapshot } = useWorkspace();
	const navigate = useNavigate();

	const handleAnalyze = async () => {
		if (!tab.url.trim()) return;
		onUpdate({ isAnalyzing: true, analyzeError: null, metadata: null });
		try {
			const metadata = await invoke<Metadata>("analyze_url", { url: tab.url });
			onUpdate({
				isAnalyzing: false,
				metadata,
				downloadMode: "vod",
				vodOptions: {},
				chatOptions: {},
			});
		} catch (e) {
			onUpdate({ isAnalyzing: false, analyzeError: String(e) });
		}
	};

	const handleDispatch = async () => {
		const taskId = crypto.randomUUID();
		const { metadata, url, downloadMode, vodOptions, chatOptions } = tab;
		if (!metadata) return;

		try {
			if (downloadMode === "vod") {
				await invoke("queue_vod_download", {
					id: taskId,
					url,
					options: vodOptions,
				});
			} else {
				await invoke("queue_chat_download", {
					id: taskId,
					url,
					options: chatOptions,
				});
			}

			registerTaskSnapshot({
				tabId: tab.id,
				taskId,
				url,
				jsonFilePath: "",
				vodOptions,
				chatOptions,
				renderOptions: tab.renderOptions,
				downloadMode,
				metadata,
			});

			onUpdate({ activeTaskId: taskId });
			navigate({ to: "/queue" });
		} catch (e) {
			console.error("Dispatch failed:", e);
		}
	};

	const {
		metadata,
		isAnalyzing,
		analyzeError,
		downloadMode,
		vodOptions,
		chatOptions,
	} = tab;

	const videoFormats = metadata?.formats.filter((f) => f.hasVideo) ?? [];
	const audioFormats = metadata?.formats.filter((f) => f.hasAudio) ?? [];

	return (
		<div className="space-y-5">
			{/* URL Input */}
			<div className="flex gap-2">
				<input
					type="text"
					placeholder="https://twitch.tv/... or YouTube URL"
					value={tab.url}
					onChange={(e) => onUpdate({ url: e.target.value })}
					onKeyDown={(e) => e.key === "Enter" && handleAnalyze()}
					className="flex-1 bg-neutral-900 border border-neutral-700 rounded-lg px-4 py-2.5 text-sm text-neutral-200 placeholder-neutral-600 focus:outline-none focus:border-indigo-500 focus:ring-1 focus:ring-indigo-500/40 transition-colors"
				/>
				<button
					onClick={handleAnalyze}
					disabled={isAnalyzing || !tab.url.trim()}
					className="px-4 py-2.5 bg-indigo-600 hover:bg-indigo-500 disabled:bg-neutral-800 disabled:text-neutral-600 text-white text-sm font-medium rounded-lg transition-colors whitespace-nowrap"
				>
					{isAnalyzing ? (
						<span className="flex items-center gap-2">
							<span className="w-3.5 h-3.5 border-2 border-white/30 border-t-white rounded-full animate-spin" />
							Analyzing…
						</span>
					) : (
						"Analyze Target Stream"
					)}
				</button>
			</div>

			{analyzeError && (
				<div className="bg-red-950/40 border border-red-800/50 rounded-lg px-4 py-3 text-sm text-red-400">
					{analyzeError}
				</div>
			)}

			{/* Metadata Card */}
			{metadata && (
				<div className="bg-neutral-900/60 border border-neutral-800 rounded-xl overflow-hidden">
					<div className="flex gap-4 p-4">
						{metadata.thumbnail && (
							<img
								src={metadata.thumbnail}
								alt={metadata.title}
								className="w-32 h-20 object-cover rounded-lg flex-shrink-0 bg-neutral-800"
							/>
						)}
						<div className="flex-1 min-w-0 space-y-1">
							<h3 className="text-sm font-semibold text-neutral-100 line-clamp-2">
								{metadata.title}
							</h3>
							<div className="flex flex-wrap gap-x-3 gap-y-0.5">
								<span className="text-xs text-neutral-500">
									{formatDuration(metadata.duration)}
								</span>
								{metadata.isLive && (
									<span className="text-xs text-red-400 font-medium flex items-center gap-1">
										<span className="w-1.5 h-1.5 rounded-full bg-red-400 animate-pulse" />
										LIVE
									</span>
								)}
								{metadata.wasLive && (
									<span className="text-xs text-amber-500">VOD</span>
								)}
							</div>
							<p className="text-xs text-neutral-600 truncate">
								{metadata.originalUrl}
							</p>
						</div>
					</div>

					{/* Mode Toggle */}
					{metadata.isChatSupported && (
						<div className="px-4 pb-4">
							<div className="flex bg-neutral-950 rounded-lg p-0.5 w-fit">
								{(["vod", "chat"] as DownloadMode[]).map((mode) => (
									<button
										key={mode}
										onClick={() => onUpdate({ downloadMode: mode })}
										className={`px-4 py-1.5 text-xs font-medium rounded-md transition-all ${
											downloadMode === mode
												? "bg-indigo-600 text-white shadow"
												: "text-neutral-500 hover:text-neutral-300"
										}`}
									>
										{mode === "vod" ? "Media VOD Download" : "Chat Extraction"}
									</button>
								))}
							</div>
						</div>
					)}
				</div>
			)}

			{/* VOD Config */}
			{metadata && downloadMode === "vod" && (
				<div className="space-y-4">
					<div className="grid grid-cols-2 gap-3">
						<div>
							<label className="block text-xs font-medium text-neutral-500 mb-1.5 uppercase tracking-wide">
								Video Format
							</label>
							<select
								value={vodOptions.videoFormatId ?? ""}
								onChange={(e) =>
									onUpdate({
										vodOptions: {
											...vodOptions,
											videoFormatId: e.target.value || undefined,
										},
									})
								}
								className="w-full bg-neutral-900 border border-neutral-700 rounded-lg px-3 py-2 text-sm text-neutral-200 focus:outline-none focus:border-indigo-500 transition-colors"
							>
								<option value="">Auto-select</option>
								{videoFormats.map((f) => (
									<option key={f.formatId} value={f.formatId}>
										{f.uiLabel}
									</option>
								))}
							</select>
						</div>
						<div>
							<label className="block text-xs font-medium text-neutral-500 mb-1.5 uppercase tracking-wide">
								Audio Format
							</label>
							<select
								value={vodOptions.audioFormatId ?? ""}
								onChange={(e) =>
									onUpdate({
										vodOptions: {
											...vodOptions,
											audioFormatId: e.target.value || undefined,
										},
									})
								}
								className="w-full bg-neutral-900 border border-neutral-700 rounded-lg px-3 py-2 text-sm text-neutral-200 focus:outline-none focus:border-indigo-500 transition-colors"
							>
								<option value="">Auto-select</option>
								{audioFormats.map((f) => (
									<option key={f.formatId} value={f.formatId}>
										{f.uiLabel}
									</option>
								))}
							</select>
						</div>
					</div>

					<label className="flex items-center gap-2 cursor-pointer">
						<input
							type="checkbox"
							checked={vodOptions.audioOnly ?? false}
							onChange={(e) =>
								onUpdate({
									vodOptions: { ...vodOptions, audioOnly: e.target.checked },
								})
							}
							className="w-4 h-4 rounded accent-indigo-500"
						/>
						<span className="text-sm text-neutral-400">Audio Only</span>
					</label>

					<div className="grid grid-cols-2 gap-3">
						<div>
							<label className="block text-xs font-medium text-neutral-500 mb-1.5 uppercase tracking-wide">
								Output Filename
							</label>
							<input
								type="text"
								placeholder="%(title)s.%(ext)s"
								value={vodOptions.fileName ?? ""}
								onChange={(e) =>
									onUpdate({
										vodOptions: {
											...vodOptions,
											fileName: e.target.value || undefined,
										},
									})
								}
								className="w-full bg-neutral-900 border border-neutral-700 rounded-lg px-3 py-2 text-sm text-neutral-200 placeholder-neutral-700 focus:outline-none focus:border-indigo-500 transition-colors"
							/>
						</div>
						<div>
							<label className="block text-xs font-medium text-neutral-500 mb-1.5 uppercase tracking-wide">
								Save Folder
							</label>
							<input
								type="text"
								placeholder="/downloads"
								value={vodOptions.saveFolder ?? ""}
								onChange={(e) =>
									onUpdate({
										vodOptions: {
											...vodOptions,
											saveFolder: e.target.value || undefined,
										},
									})
								}
								className="w-full bg-neutral-900 border border-neutral-700 rounded-lg px-3 py-2 text-sm text-neutral-200 placeholder-neutral-700 focus:outline-none focus:border-indigo-500 transition-colors"
							/>
						</div>
					</div>

					<div className="grid grid-cols-2 gap-3">
						<div>
							<label className="block text-xs font-medium text-neutral-500 mb-1.5 uppercase tracking-wide">
								Start (ms)
							</label>
							<input
								type="number"
								placeholder="0"
								value={vodOptions.startMs ?? ""}
								onChange={(e) =>
									onUpdate({
										vodOptions: {
											...vodOptions,
											startMs: e.target.value
												? Number(e.target.value)
												: undefined,
										},
									})
								}
								className="w-full bg-neutral-900 border border-neutral-700 rounded-lg px-3 py-2 text-sm text-neutral-200 placeholder-neutral-700 focus:outline-none focus:border-indigo-500 transition-colors"
							/>
						</div>
						<div>
							<label className="block text-xs font-medium text-neutral-500 mb-1.5 uppercase tracking-wide">
								End (ms)
							</label>
							<input
								type="number"
								placeholder="End of stream"
								value={vodOptions.endMs ?? ""}
								onChange={(e) =>
									onUpdate({
										vodOptions: {
											...vodOptions,
											endMs: e.target.value
												? Number(e.target.value)
												: undefined,
										},
									})
								}
								className="w-full bg-neutral-900 border border-neutral-700 rounded-lg px-3 py-2 text-sm text-neutral-200 placeholder-neutral-700 focus:outline-none focus:border-indigo-500 transition-colors"
							/>
						</div>
					</div>

					<div className="grid grid-cols-2 gap-3">
						<div>
							<label className="block text-xs font-medium text-neutral-500 mb-1.5 uppercase tracking-wide">
								Threads
							</label>
							<input
								type="number"
								placeholder="Auto"
								min={1}
								max={32}
								value={vodOptions.threads ?? ""}
								onChange={(e) =>
									onUpdate({
										vodOptions: {
											...vodOptions,
											threads: e.target.value
												? Number(e.target.value)
												: undefined,
										},
									})
								}
								className="w-full bg-neutral-900 border border-neutral-700 rounded-lg px-3 py-2 text-sm text-neutral-200 placeholder-neutral-700 focus:outline-none focus:border-indigo-500 transition-colors"
							/>
						</div>
						<div>
							<label className="block text-xs font-medium text-neutral-500 mb-1.5 uppercase tracking-wide">
								Rate Limit
							</label>
							<input
								type="text"
								placeholder="e.g. 5M"
								value={vodOptions.limitRate ?? ""}
								onChange={(e) =>
									onUpdate({
										vodOptions: {
											...vodOptions,
											limitRate: e.target.value || undefined,
										},
									})
								}
								className="w-full bg-neutral-900 border border-neutral-700 rounded-lg px-3 py-2 text-sm text-neutral-200 placeholder-neutral-700 focus:outline-none focus:border-indigo-500 transition-colors"
							/>
						</div>
					</div>
				</div>
			)}

			{/* Chat Config */}
			{metadata && downloadMode === "chat" && (
				<div className="space-y-4">
					<div className="grid grid-cols-2 gap-3">
						<div>
							<label className="block text-xs font-medium text-neutral-500 mb-1.5 uppercase tracking-wide">
								Save Folder
							</label>
							<input
								type="text"
								placeholder="/downloads"
								value={chatOptions.saveFolder ?? ""}
								onChange={(e) =>
									onUpdate({
										chatOptions: {
											...chatOptions,
											saveFolder: e.target.value || undefined,
										},
									})
								}
								className="w-full bg-neutral-900 border border-neutral-700 rounded-lg px-3 py-2 text-sm text-neutral-200 placeholder-neutral-700 focus:outline-none focus:border-indigo-500 transition-colors"
							/>
						</div>
						<div>
							<label className="block text-xs font-medium text-neutral-500 mb-1.5 uppercase tracking-wide">
								Output Filename
							</label>
							<input
								type="text"
								placeholder="chat.jsonl"
								value={chatOptions.fileName ?? ""}
								onChange={(e) =>
									onUpdate({
										chatOptions: {
											...chatOptions,
											fileName: e.target.value || undefined,
										},
									})
								}
								className="w-full bg-neutral-900 border border-neutral-700 rounded-lg px-3 py-2 text-sm text-neutral-200 placeholder-neutral-700 focus:outline-none focus:border-indigo-500 transition-colors"
							/>
						</div>
					</div>

					<div className="grid grid-cols-2 gap-3">
						<div>
							<label className="block text-xs font-medium text-neutral-500 mb-1.5 uppercase tracking-wide">
								Max Retries
							</label>
							<input
								type="number"
								placeholder="3"
								min={0}
								value={chatOptions.maxRetries ?? ""}
								onChange={(e) =>
									onUpdate({
										chatOptions: {
											...chatOptions,
											maxRetries: e.target.value
												? Number(e.target.value)
												: undefined,
										},
									})
								}
								className="w-full bg-neutral-900 border border-neutral-700 rounded-lg px-3 py-2 text-sm text-neutral-200 placeholder-neutral-700 focus:outline-none focus:border-indigo-500 transition-colors"
							/>
						</div>
						<div>
							<label className="block text-xs font-medium text-neutral-500 mb-1.5 uppercase tracking-wide">
								Kick Concurrency
							</label>
							<input
								type="number"
								placeholder="4"
								min={1}
								value={chatOptions.kickConcurrency ?? ""}
								onChange={(e) =>
									onUpdate({
										chatOptions: {
											...chatOptions,
											kickConcurrency: e.target.value
												? Number(e.target.value)
												: undefined,
										},
									})
								}
								className="w-full bg-neutral-900 border border-neutral-700 rounded-lg px-3 py-2 text-sm text-neutral-200 placeholder-neutral-700 focus:outline-none focus:border-indigo-500 transition-colors"
							/>
						</div>
					</div>

					<div className="grid grid-cols-2 gap-3">
						<div>
							<label className="block text-xs font-medium text-neutral-500 mb-1.5 uppercase tracking-wide">
								Start (ms)
							</label>
							<input
								type="number"
								placeholder="0"
								value={chatOptions.startMs ?? ""}
								onChange={(e) =>
									onUpdate({
										chatOptions: {
											...chatOptions,
											startMs: e.target.value
												? Number(e.target.value)
												: undefined,
										},
									})
								}
								className="w-full bg-neutral-900 border border-neutral-700 rounded-lg px-3 py-2 text-sm text-neutral-200 placeholder-neutral-700 focus:outline-none focus:border-indigo-500 transition-colors"
							/>
						</div>
						<div>
							<label className="block text-xs font-medium text-neutral-500 mb-1.5 uppercase tracking-wide">
								End (ms)
							</label>
							<input
								type="number"
								placeholder="End"
								value={chatOptions.endMs ?? ""}
								onChange={(e) =>
									onUpdate({
										chatOptions: {
											...chatOptions,
											endMs: e.target.value
												? Number(e.target.value)
												: undefined,
										},
									})
								}
								className="w-full bg-neutral-900 border border-neutral-700 rounded-lg px-3 py-2 text-sm text-neutral-200 placeholder-neutral-700 focus:outline-none focus:border-indigo-500 transition-colors"
							/>
						</div>
					</div>
				</div>
			)}

			{/* Dispatch */}
			{metadata && (
				<button
					onClick={handleDispatch}
					className="w-full py-3 bg-indigo-600 hover:bg-indigo-500 text-white text-sm font-semibold rounded-xl transition-colors shadow-lg shadow-indigo-900/40"
				>
					Initialize Background Operation
				</button>
			)}
		</div>
	);
}
