interface ProgressBarProps {
	value: number; // 0–100
	className?: string;
	color?: string;
}

export function ProgressBar({
	value,
	className = "",
	color,
}: ProgressBarProps) {
	const pct = Math.max(0, Math.min(100, value));
	return (
		<div
			className={`w-full h-1.5 bg-neutral-800 rounded-full overflow-hidden ${className}`}
		>
			<div
				className="h-full rounded-full transition-all duration-300"
				style={{
					width: `${pct}%`,
					background: color ?? "linear-gradient(90deg, #6366f1, #8b5cf6)",
				}}
			/>
		</div>
	);
}
