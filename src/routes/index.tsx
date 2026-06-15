import { createFileRoute } from "@tanstack/react-router";
import { Plus, X, Loader2, AlertCircle } from "lucide-react";
import { useTabStore } from "@/store/useTabStore.ts";
import { StreamInput } from "@/features/downloads/components/StreamInput.tsx";
import { StreamDetails } from "@/features/downloads/components/StreamDetails.tsx";

export const Route = createFileRoute("/")({
	component: IndexPage,
});

function IndexPage() {
	const { tabs, activeTabId, addTab, closeTab, setActiveTab } = useTabStore();

	if (!activeTabId && tabs.length > 0) {
		setActiveTab(tabs[0].id);
	}

	const activeTab = tabs.find((t) => t.id === activeTabId);

	return (
		<div className="flex h-screen w-full flex-col bg-background text-foreground overflow-hidden">
			{/* TAB BAR */}
			<div className="flex items-center gap-1 bg-muted px-2 pt-2 border-b border-border shadow-sm">
				{tabs.map((tab) => (
					<button
						key={tab.id}
						onClick={() => setActiveTab(tab.id)}
						className={`group flex items-center gap-2 rounded-t-md border-x border-t px-4 py-2 text-sm transition-colors ${
							activeTabId === tab.id
								? "border-border bg-card text-card-foreground shadow-sm"
								: "border-transparent text-muted-foreground hover:bg-accent/50"
						}`}
					>
						<span className="max-w-30 truncate">{tab.title}</span>
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
					className="ml-1 rounded-md p-1.5 text-muted-foreground hover:bg-accent hover:text-accent-foreground transition-colors"
				>
					<Plus className="h-4 w-4" />
				</button>
			</div>

			{/* TAB CONTENT VIEW - Added min-h-0 to fix grid layout scroll trap */}
			<div className="flex-1 bg-card min-h-0">
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
								useTabStore
									.getState()
									.updateTab(activeTab.id, { status: "idle" })
							}
							className="mt-4 rounded-md border border-input bg-transparent px-4 py-2 text-sm font-medium hover:bg-accent text-foreground"
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
