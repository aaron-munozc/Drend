import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";

interface ToolStatus {
	available: boolean;
	loading: boolean;
	installing: boolean;
}

function StatusDot({ ok, loading }: { ok: boolean; loading: boolean }) {
	if (loading)
		return (
			<span className="w-2 h-2 rounded-full border border-neutral-500 border-t-transparent animate-spin inline-block" />
		);
	return (
		<span
			className={`w-2 h-2 rounded-full inline-block ${ok ? "bg-emerald-400" : "bg-red-400"}`}
		/>
	);
}

export function DiagnosticTray() {
	const [open, setOpen] = useState(false);
	const [ytdlp, setYtdlp] = useState<ToolStatus>({
		available: false,
		loading: true,
		installing: false,
	});
	const [ffmpeg, setFfmpeg] = useState<ToolStatus>({
		available: false,
		loading: true,
		installing: false,
	});
	const [apiRunning, setApiRunning] = useState(false);
	const [apiLoading, setApiLoading] = useState(false);

	useEffect(() => {
		if (!open) return;
		checkAll();
	}, [open]);

	const checkAll = async () => {
		setYtdlp((s) => ({ ...s, loading: true }));
		setFfmpeg((s) => ({ ...s, loading: true }));
		try {
			const [y, f] = await Promise.all([
				invoke<boolean>("check_ytdlp"),
				invoke<boolean>("check_ffmpeg"),
			]);
			setYtdlp({ available: y, loading: false, installing: false });
			setFfmpeg({ available: f, loading: false, installing: false });
		} catch {
			setYtdlp({ available: false, loading: false, installing: false });
			setFfmpeg({ available: false, loading: false, installing: false });
		}
	};

	const install = async (tool: "ytdlp" | "ffmpeg") => {
		const set = tool === "ytdlp" ? setYtdlp : setFfmpeg;
		const cmd = tool === "ytdlp" ? "install_ytdlp" : "install_ffmpeg";
		set((s) => ({ ...s, installing: true }));
		try {
			await invoke(cmd);
			set({ available: true, loading: false, installing: false });
		} catch {
			set((s) => ({ ...s, installing: false }));
		}
	};

	const toggleApi = async () => {
		setApiLoading(true);
		try {
			if (apiRunning) {
				await invoke("stop_api_server");
				setApiRunning(false);
			} else {
				await invoke("start_api_server");
				setApiRunning(true);
			}
		} catch {
			//
		} finally {
			setApiLoading(false);
		}
	};

	const anyIssue =
		!ytdlp.loading &&
		!ffmpeg.loading &&
		(!ytdlp.available || !ffmpeg.available);

	return (
		<div className="relative">
			<button
				onClick={() => setOpen((o) => !o)}
				className={`flex items-center gap-2 px-3 py-1.5 rounded-lg text-xs font-medium transition-colors border ${
					anyIssue
						? "bg-red-950/40 border-red-800/50 text-red-400 hover:bg-red-900/50"
						: "bg-neutral-800 border-neutral-700 text-neutral-400 hover:text-neutral-200"
				}`}
				aria-label="System diagnostics"
			>
				<svg
					width="14"
					height="14"
					viewBox="0 0 14 14"
					fill="none"
					stroke="currentColor"
					strokeWidth="1.5"
				>
					<circle cx="7" cy="7" r="5.5" />
					<path d="M7 4.5v3M7 9.5v.5" strokeLinecap="round" />
				</svg>
				Diagnostics
				{anyIssue && (
					<span className="w-1.5 h-1.5 rounded-full bg-red-400 animate-pulse" />
				)}
			</button>

			{open && (
				<>
					<div className="fixed inset-0 z-10" onClick={() => setOpen(false)} />
					<div className="absolute right-0 top-full mt-2 w-72 bg-neutral-900 border border-neutral-800 rounded-xl shadow-2xl z-20 overflow-hidden">
						<div className="px-4 py-3 border-b border-neutral-800">
							<h3 className="text-xs font-semibold text-neutral-300 uppercase tracking-widest">
								System Diagnostics
							</h3>
						</div>

						<div className="p-3 space-y-2">
							{/* yt-dlp */}
							<div className="flex items-center justify-between bg-neutral-950/60 rounded-lg px-3 py-2.5">
								<div className="flex items-center gap-2">
									<StatusDot ok={ytdlp.available} loading={ytdlp.loading} />
									<span className="text-xs text-neutral-300 font-medium">
										yt-dlp
									</span>
									<span className="text-xs text-neutral-600">
										video downloader
									</span>
								</div>
								{!ytdlp.loading && !ytdlp.available && (
									<button
										onClick={() => install("ytdlp")}
										disabled={ytdlp.installing}
										className="text-xs px-2 py-1 bg-indigo-600 hover:bg-indigo-500 disabled:opacity-50 text-white rounded-md transition-colors"
									>
										{ytdlp.installing ? "Installing…" : "Install"}
									</button>
								)}
							</div>

							{/* ffmpeg */}
							<div className="flex items-center justify-between bg-neutral-950/60 rounded-lg px-3 py-2.5">
								<div className="flex items-center gap-2">
									<StatusDot ok={ffmpeg.available} loading={ffmpeg.loading} />
									<span className="text-xs text-neutral-300 font-medium">
										ffmpeg
									</span>
									<span className="text-xs text-neutral-600">
										media processor
									</span>
								</div>
								{!ffmpeg.loading && !ffmpeg.available && (
									<button
										onClick={() => install("ffmpeg")}
										disabled={ffmpeg.installing}
										className="text-xs px-2 py-1 bg-indigo-600 hover:bg-indigo-500 disabled:opacity-50 text-white rounded-md transition-colors"
									>
										{ffmpeg.installing ? "Installing…" : "Install"}
									</button>
								)}
							</div>

							{/* API Server */}
							<div className="flex items-center justify-between bg-neutral-950/60 rounded-lg px-3 py-2.5">
								<div className="flex items-center gap-2">
									<span
										className={`w-2 h-2 rounded-full ${apiRunning ? "bg-emerald-400" : "bg-neutral-600"}`}
									/>
									<span className="text-xs text-neutral-300 font-medium">
										Axum API
									</span>
									<span className="text-xs text-neutral-600">local server</span>
								</div>
								<button
									onClick={toggleApi}
									disabled={apiLoading}
									className={`text-xs px-2 py-1 rounded-md transition-colors disabled:opacity-50 ${
										apiRunning
											? "bg-red-950/60 hover:bg-red-900/60 text-red-400 border border-red-800/40"
											: "bg-emerald-900/40 hover:bg-emerald-800/40 text-emerald-400 border border-emerald-800/40"
									}`}
								>
									{apiLoading ? "…" : apiRunning ? "Stop" : "Start"}
								</button>
							</div>
						</div>

						<div className="px-3 pb-3">
							<button
								onClick={checkAll}
								className="w-full py-1.5 text-xs text-neutral-500 hover:text-neutral-300 transition-colors"
							>
								Refresh checks
							</button>
						</div>
					</div>
				</>
			)}
		</div>
	);
}
