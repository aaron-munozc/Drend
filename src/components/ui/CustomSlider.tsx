interface CustomSliderProps {
	label: string;
	value: number;
	min: number;
	max: number;
	step?: number;
	unit?: string;
	onChange: (value: number) => void;
}

export function CustomSlider({
	label,
	value,
	min,
	max,
	step = 1,
	unit = "",
	onChange,
}: CustomSliderProps) {
	return (
		<div className="space-y-1">
			<div className="flex justify-between items-center">
				<label className="text-xs font-medium text-neutral-400 uppercase tracking-wide">
					{label}
				</label>
				<span className="text-xs text-neutral-300 font-mono">
					{value}
					{unit}
				</span>
			</div>
			<div className="flex items-center gap-3">
				<input
					type="range"
					min={min}
					max={max}
					step={step}
					value={value}
					onChange={(e) => onChange(Number(e.target.value))}
					className="flex-1 h-1 accent-indigo-500"
				/>
				<input
					type="number"
					min={min}
					max={max}
					step={step}
					value={value}
					onChange={(e) =>
						onChange(Math.max(min, Math.min(max, Number(e.target.value))))
					}
					className="w-16 text-xs bg-neutral-800 border border-neutral-700 rounded px-2 py-1 text-neutral-200 text-center font-mono"
				/>
			</div>
		</div>
	);
}
