import { createRootRoute, Outlet } from "@tanstack/react-router";
import { Sidebar } from "../components/Sidebar";
import { DiagnosticTray } from "../components/DiagnosticTray";

export const Route = createRootRoute({
	component: RootLayout,
});

function RootLayout() {
	return (
		<div className="flex h-screen w-screen bg-neutral-950 text-neutral-200 overflow-hidden select-none">
			<Sidebar />
			<div className="flex flex-col flex-1 min-w-0">
				{/* Top bar */}
				<header className="h-10 flex items-center justify-end px-4 border-b border-neutral-800/60 shrink-0 bg-neutral-950/80 backdrop-blur-sm">
					<DiagnosticTray />
				</header>
				{/* Main content */}
				<main className="flex-1 min-h-0 overflow-hidden">
					<Outlet />
				</main>
			</div>
		</div>
	);
}