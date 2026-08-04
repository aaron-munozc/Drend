import { Link, useLocation } from "@tanstack/react-router";

export function Sidebar() {
	const location = useLocation();

	const navItems = [
		{
			to: "/",
			label: "Workspace",
			icon: (
				<svg
					width="16"
					height="16"
					viewBox="0 0 16 16"
					fill="none"
					stroke="currentColor"
					strokeWidth="1.5"
				>
					<rect x="1.5" y="1.5" width="5.5" height="5.5" rx="1" />
					<rect x="9" y="1.5" width="5.5" height="5.5" rx="1" />
					<rect x="1.5" y="9" width="5.5" height="5.5" rx="1" />
					<rect x="9" y="9" width="5.5" height="5.5" rx="1" />
				</svg>
			),
		},
		{
			to: "/queue",
			label: "Queue",
			icon: (
				<svg
					width="16"
					height="16"
					viewBox="0 0 16 16"
					fill="none"
					stroke="currentColor"
					strokeWidth="1.5"
				>
					<path d="M2 4h12M2 8h8M2 12h10" strokeLinecap="round" />
				</svg>
			),
		},
	];

	return (
		<aside className="w-14 flex flex-col items-center border-r border-neutral-800/60 bg-neutral-950 py-4 gap-1 flex-shrink-0">
			{/* Logo mark */}
			<div className="w-8 h-8 rounded-lg bg-gradient-to-br from-indigo-600 to-violet-600 flex items-center justify-center mb-4 flex-shrink-0">
				<svg width="16" height="16" viewBox="0 0 16 16" fill="white">
					<path d="M3 3h4v4H3zM9 3h4v4H9zM3 9h4v4H3zM9 9h4v4H9z" opacity=".6" />
				</svg>
			</div>

			{navItems.map((item) => {
				const active =
					item.to === "/"
						? location.pathname === "/"
						: location.pathname.startsWith(item.to);
				return (
					<Link
						key={item.to}
						to={item.to}
						className={`w-10 h-10 flex items-center justify-center rounded-lg transition-colors ${
							active
								? "bg-indigo-600/20 text-indigo-400"
								: "text-neutral-600 hover:text-neutral-300 hover:bg-neutral-800"
						}`}
						title={item.label}
					>
						{item.icon}
					</Link>
				);
			})}
		</aside>
	);
}
