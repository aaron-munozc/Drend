import { createFileRoute } from "@tanstack/react-router";
import {
	Plus,
	X,
	Loader2,
	AlertCircle,
	LayoutDashboard,
	Film,
	ListOrdered,
	ChevronLeft,
	ChevronRight,
} from "lucide-react";
import { useDownloadTabStore } from "@/store/useDownloadTabStore.ts";
import { useAppStore } from "@/store/useAppStore.ts";
import { StreamInput } from "@/features/downloads/components/StreamInput.tsx";
import { StreamDetails } from "@/features/downloads/components/StreamDetails.tsx";
import { QueueView } from "@/features/queue/components/QueueView.tsx";
import { RenderView } from "@/features/render/components/RenderView.tsx";

export const Route = createFileRoute("/")({
	component: IndexPage,
});

function IndexPage() {
	const { activeView, setActiveView, isSidebarCollapsed, toggleSidebar } =
		useAppStore();

	return (
		<div className="flex h-screen w-full bg-background text-foreground overflow-hidden">
			{/* SIDEBAR */}
			<aside
				className={`${
					isSidebarCollapsed ? "w-16" : "w-64"
				} flex-shrink-0 border-r border-border bg-card flex flex-col justify-between transition-all duration-300 ease-in-out z-20`}
			>
				<div className="p-4 space-y-6 overflow-hidden">
					<div
						className={`flex items-center ${isSidebarCollapsed ? "justify-center" : "justify-start"} gap-3 px-2 transition-all`}
					>
						<div className="h-8 w-8 min-w-[32px] rounded-lg bg-primary flex items-center justify-center text-primary-foreground font-bold shadow-md">
							S
						</div>
						{!isSidebarCollapsed && (
							<span className="font-bold tracking-wider whitespace-nowrap">
								STREAMER
							</span>
						)}
					</div>

					<nav className="space-y-2">
						<SidebarButton
							icon={<LayoutDashboard />}
							label="Downloads"
							isActive={activeView === "downloads"}
							isCollapsed={isSidebarCollapsed}
							onClick={() => setActiveView("downloads")}
						/>
						<SidebarButton
							icon={<Film />}
							label="Chat Render"
							isActive={activeView === "render"}
							isCollapsed={isSidebarCollapsed}
							onClick={() => setActiveView("render")}
						/>
						<SidebarButton
							icon={<ListOrdered />}
							label="Task Queue"
							isActive={activeView === "queue"}
							isCollapsed={isSidebarCollapsed}
							onClick={() => setActiveView("queue")}
						/>
					</nav>
				</div>

				<div className="p-4 border-t border-border flex justify-center">
					<button
						onClick={toggleSidebar}
						className="p-2 rounded-lg text-muted-foreground hover:bg-muted hover:text-foreground transition-colors"
					>
						{isSidebarCollapsed ? (
							<ChevronRight className="w-5 h-5" />
						) : (
							<ChevronLeft className="w-5 h-5" />
						)}
					</button>
				</div>
			</aside>

			{/* MAIN CONTENT AREA */}
			<main className="flex-1 flex flex-col min-w-0 bg-background relative z-10">
				{activeView === "downloads" && <DownloadsView />}
				{activeView === "render" && <RenderView />}
				{activeView === "queue" && <QueueView />}
			</main>
		</div>
	);
}

// --- SUB-COMPONENTS ---

function SidebarButton({ icon, label, isActive, isCollapsed, onClick }: any) {
	return (
		<button
			onClick={onClick}
			title={isCollapsed ? label : undefined}
			className={`w-full flex items-center ${
				isCollapsed ? "justify-center px-0" : "justify-start px-3"
			} gap-3 py-3 rounded-xl transition-all duration-200 group ${
				isActive
					? "bg-primary/10 text-primary font-semibold"
					: "text-muted-foreground hover:bg-muted hover:text-foreground"
			}`}
		>
			<span
				className={`[&>svg]:w-5 [&>svg]:h-5 shrink-0 ${isActive ? "text-primary" : "group-hover:text-foreground"}`}
			>
				{icon}
			</span>
			{!isCollapsed && <span className="whitespace-nowrap">{label}</span>}
		</button>
	);
}

// Your existing downloads view component (encapsulated)
function DownloadsView() {
	const { tabs, activeTabId, addTab, closeTab, setActiveTab } =
		useDownloadTabStore();
	if (!activeTabId && tabs.length > 0) setActiveTab(tabs[0].id);
	const activeTab = tabs.find((t) => t.id === activeTabId);

	return (
		<div className="flex h-full w-full flex-col overflow-hidden">
			{/* TAB BAR */}
			<div className="flex items-center gap-1 bg-muted px-2 pt-2 border-b border-border shadow-sm overflow-x-auto no-scrollbar">
				{tabs.map((tab) => (
					<button
						key={tab.id}
						onClick={() => setActiveTab(tab.id)}
						className={`group flex shrink-0 items-center gap-2 rounded-t-md border-x border-t px-4 py-2 text-sm transition-colors ${
							activeTabId === tab.id
								? "border-border bg-card text-card-foreground shadow-sm"
								: "border-transparent text-muted-foreground hover:bg-accent/50"
						}`}
					>
						<span className="max-w-[150px] truncate">{tab.title}</span>
						<div
							role="button"
							tabIndex={0}
							onClick={(e) => {
								e.stopPropagation();
								closeTab(tab.id);
							}}
							className="rounded-sm p-0.5 opacity-0 hover:bg-muted group-hover:opacity-100 transition-opacity"
						>
							<X className="h-3 w-3" />
						</div>
					</button>
				))}
				<button
					onClick={addTab}
					className="ml-1 shrink-0 rounded-md p-1.5 text-muted-foreground hover:bg-accent transition-colors"
				>
					<Plus className="h-4 w-4" />
				</button>
			</div>

			{/* TAB CONTENT */}
			<div className="flex-1 bg-card min-h-0 overflow-y-auto">
				{activeTab?.status === "idle" && <StreamInput tabId={activeTab.id} />}
				{activeTab?.status === "loading" && (
					<div className="flex h-full items-center justify-center flex-col gap-3">
						<Loader2 className="h-8 w-8 animate-spin text-primary" />
						<p className="text-sm text-muted-foreground">
							Extracting metadata...
						</p>
					</div>
				)}
				{activeTab?.status === "error" && (
					<div className="flex h-full items-center justify-center flex-col gap-3 text-destructive p-6 text-center">
						<AlertCircle className="h-10 w-10" />
						<h2 className="text-lg font-semibold">Analysis Failed</h2>
						<p className="text-sm max-w-md">{activeTab.error}</p>
						<button
							onClick={() =>
								useDownloadTabStore
									.getState()
									.updateTab(activeTab.id, { status: "idle" })
							}
							className="mt-4 rounded-md border bg-transparent px-4 py-2 text-sm text-foreground hover:bg-accent"
						>
							Try Again
						</button>
					</div>
				)}
				{activeTab?.status === "analyzed" && <StreamDetails tab={activeTab} />}
			</div>
		</div>
	);
}
