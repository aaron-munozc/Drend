import { useState } from "react";
import {TabState} from "@/stores/useWorkspaceStore.ts";
import { DownloadForm } from "./DownloadForm";
import { RenderForm } from "./RenderForm";

type Section = "download" | "render";

interface TabWorkspaceProps {
	tab: TabState;
	onUpdate: (patch: Partial<TabState>) => void;
}

export function TabWorkspace({ tab, onUpdate }: TabWorkspaceProps) {
	const [section, setSection] = useState<Section>("download");

	return (
		<div className="flex flex-col h-full">
			{/* Section Toggle */}
			<div className="flex border-b border-neutral-800 px-6">
				<button
					onClick={() => setSection("download")}
					className={`px-4 py-3 text-sm font-medium border-b-2 transition-colors -mb-px ${
						section === "download"
							? "border-indigo-500 text-indigo-400"
							: "border-transparent text-neutral-500 hover:text-neutral-300"
					}`}
				>
					VOD / Chat Download
				</button>
				<button
					onClick={() => setSection("render")}
					className={`px-4 py-3 text-sm font-medium border-b-2 transition-colors -mb-px ${
						section === "render"
							? "border-violet-500 text-violet-400"
							: "border-transparent text-neutral-500 hover:text-neutral-300"
					}`}
				>
					Chat Renderer
				</button>
			</div>

			{/* Content */}
			<div className="flex-1 overflow-y-auto p-6">
				{section === "download" ? (
					<DownloadForm tab={tab} onUpdate={onUpdate} />
				) : (
					<RenderForm tab={tab} onUpdate={onUpdate} />
				)}
			</div>
		</div>
	);
}
