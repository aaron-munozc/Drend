import { createFileRoute } from "@tanstack/react-router";
import { useState, useRef, useEffect } from "react";
import {
	TabState,
	useTabs,
	useActiveTab,
	useActiveTabId,
	useWorkspaceStore,
} from "@/stores/useWorkspaceStore.ts";
import { TabWorkspace } from "../components/TabWorkspace";

export const Route = createFileRoute("/")({
	component: IndexPage,
});

function IndexPage() {
	const tabs = useTabs();
	const activeTabId = useActiveTabId();
	const activeTab = useActiveTab();

	const setActiveTab = useWorkspaceStore((s) => s.setActiveTab);
	const addTab = useWorkspaceStore((s) => s.addTab);
	const closeTab = useWorkspaceStore((s) => s.closeTab);
	const renameTab = useWorkspaceStore((s) => s.renameTab);
	const updateTab = useWorkspaceStore((s) => s.updateTab);

	const [editingTabId, setEditingTabId] = useState<string | null>(null);
	const [editingLabel, setEditingLabel] = useState("");
	const editInputRef = useRef<HTMLInputElement>(null);

	useEffect(() => {
		if (editingTabId && editInputRef.current) {
			editInputRef.current.focus();
			editInputRef.current.select();
		}
	}, [editingTabId]);

	const handleTabDoubleClick = (tab: TabState) => {
		setEditingTabId(tab.id);
		setEditingLabel(tab.label);
	};

	const commitRename = () => {
		if (editingTabId && editingLabel.trim()) {
			renameTab(editingTabId, editingLabel.trim());
		}
		setEditingTabId(null);
	};

	return (
		<div className="flex flex-col h-full">
			{/* ── Tab Bar ─────────────────────────────────────────────────────── */}
			<div className="flex items-stretch border-b border-neutral-800 bg-neutral-950 overflow-x-auto flex-shrink-0 h-10">
				<div className="flex items-stretch min-w-0">
					{tabs.map((tab) => {
						const isActive = tab.id === activeTabId;
						const isEditing = tab.id === editingTabId;

						return (
							<div
								key={tab.id}
								onClick={() => !isEditing && setActiveTab(tab.id)}
								onDoubleClick={() => handleTabDoubleClick(tab)}
								className={`
                  group relative flex items-center gap-2 px-3 py-0 border-r border-neutral-800
                  cursor-pointer select-none min-w-0 max-w-52 flex-shrink-0 transition-colors duration-100
                  ${
									isActive
										? "bg-neutral-900 text-neutral-200"
										: "text-neutral-500 hover:text-neutral-300 hover:bg-neutral-900/40"
								}
                `}
							>
								{/* Active indicator */}
								{isActive && (
									<span className="absolute bottom-0 left-0 right-0 h-[2px] bg-indigo-500 rounded-t-full" />
								)}

								{/* Live task dot */}
								{tab.activeTaskId && (
									<span className="w-1.5 h-1.5 rounded-full bg-indigo-400 flex-shrink-0 animate-pulse" />
								)}

								{isEditing ? (
									<input
										ref={editInputRef}
										value={editingLabel}
										onChange={(e) => setEditingLabel(e.target.value)}
										onBlur={commitRename}
										onKeyDown={(e) => {
											if (e.key === "Enter") commitRename();
											if (e.key === "Escape") setEditingTabId(null);
										}}
										onClick={(e) => e.stopPropagation()}
										className="bg-neutral-800 text-neutral-200 text-xs rounded px-1.5 py-0.5 outline-none w-28 min-w-0 border border-indigo-500/50"
									/>
								) : (
									<span className="text-xs truncate flex-1 min-w-0">{tab.label}</span>
								)}

								{/* Close button */}
								<button
									onClick={(e) => {
										e.stopPropagation();
										closeTab(tab.id);
									}}
									className="w-4 h-4 flex items-center justify-center rounded opacity-0 group-hover:opacity-100 hover:bg-neutral-700/80 text-neutral-500 hover:text-neutral-200 transition-all flex-shrink-0 ml-0.5"
									aria-label={`Close ${tab.label}`}
								>
									<svg width="8" height="8" viewBox="0 0 8 8" fill="none">
										<path
											d="M1 1l6 6M7 1L1 7"
											stroke="currentColor"
											strokeWidth="1.5"
											strokeLinecap="round"
										/>
									</svg>
								</button>
							</div>
						);
					})}
				</div>

				{/* New Tab button */}
				<button
					onClick={() => addTab()}
					className="w-10 flex items-center justify-center text-neutral-600 hover:text-neutral-300 hover:bg-neutral-800/60 transition-colors flex-shrink-0"
					aria-label="New workspace tab"
					title="New workspace  (double-click tab to rename)"
				>
					<svg
						width="13"
						height="13"
						viewBox="0 0 14 14"
						fill="none"
						stroke="currentColor"
						strokeWidth="2"
						strokeLinecap="round"
					>
						<path d="M7 2v10M2 7h10" />
					</svg>
				</button>
			</div>

			{/* ── Content ─────────────────────────────────────────────────────── */}
			<div className="flex-1 min-h-0 overflow-hidden">
				{tabs.length === 0 ? (
					<EmptyWorkspace onAdd={() => addTab()} />
				) : activeTab ? (
					<TabWorkspace
						tab={activeTab}
						onUpdate={(patch) => updateTab(activeTab.id, patch)}
					/>
				) : null}
			</div>
		</div>
	);
}

function EmptyWorkspace({ onAdd }: { onAdd: () => void }) {
	return (
		<div className="flex flex-col items-center justify-center h-full gap-5 text-neutral-600">
			<div className="p-5 rounded-2xl border border-neutral-800 bg-neutral-900/40">
				<svg
					width="40"
					height="40"
					viewBox="0 0 48 48"
					fill="none"
					stroke="currentColor"
					strokeWidth="1.5"
				>
					<rect x="6" y="6" width="15" height="15" rx="2.5" />
					<rect x="27" y="6" width="15" height="15" rx="2.5" />
					<rect x="6" y="27" width="15" height="15" rx="2.5" />
					<rect x="27" y="27" width="15" height="15" rx="2.5" />
				</svg>
			</div>
			<div className="text-center">
				<p className="text-sm text-neutral-400 font-medium">No workspaces open</p>
				<p className="text-xs text-neutral-600 mt-1">Each workspace is an independent render session</p>
			</div>
			<button
				onClick={onAdd}
				className="px-5 py-2 bg-indigo-600 hover:bg-indigo-500 text-white text-sm font-semibold rounded-lg transition-colors shadow-lg shadow-indigo-900/30"
			>
				Open workspace
			</button>
		</div>
	);
}