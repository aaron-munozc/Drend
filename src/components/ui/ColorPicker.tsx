import type { ObjectColor } from "@/types/backend.ts";

interface ColorPickerProps {
	label: string;
	value: ObjectColor;
	onChange: (color: ObjectColor) => void;
}

const PRESETS: { label: string; color: ObjectColor }[] = [
	{ label: "Black", color: { alpha: 255, red: 0, green: 0, blue: 0 } },
	{ label: "White", color: { alpha: 255, red: 255, green: 255, blue: 255 } },
	{ label: "Gold", color: { alpha: 255, red: 255, green: 215, blue: 0 } },
	{ label: "Clear", color: { alpha: 0, red: 0, green: 0, blue: 0 } },
];

function toHex(c: ObjectColor): string {
	const r = c.red.toString(16).padStart(2, "0");
	const g = c.green.toString(16).padStart(2, "0");
	const b = c.blue.toString(16).padStart(2, "0");
	return `#${r}${g}${b}`;
}

function hexToRgb(hex: string): { red: number; green: number; blue: number } {
	const result = /^#?([a-f\d]{2})([a-f\d]{2})([a-f\d]{2})$/i.exec(hex);
	return result
		? {
				red: parseInt(result[1], 16),
				green: parseInt(result[2], 16),
				blue: parseInt(result[3], 16),
			}
		: { red: 0, green: 0, blue: 0 };
}

function Channel({
	label,
	value,
	onChange,
}: {
	label: string;
	value: number;
	onChange: (v: number) => void;
}) {
	return (
		<div className="flex items-center gap-2">
			<span className="text-xs text-neutral-500 w-4">{label}</span>
			<input
				type="range"
				min={0}
				max={255}
				value={value}
				onChange={(e) => onChange(Number(e.target.value))}
				className="flex-1 h-1 accent-indigo-500"
			/>
			<input
				type="number"
				min={0}
				max={255}
				value={value}
				onChange={(e) =>
					onChange(Math.max(0, Math.min(255, Number(e.target.value))))
				}
				className="w-12 text-xs bg-neutral-800 border border-neutral-700 rounded px-1 py-0.5 text-neutral-200 text-center"
			/>
		</div>
	);
}

export function ColorPicker({ label, value, onChange }: ColorPickerProps) {
	const set = (key: keyof ObjectColor) => (v: number) =>
		onChange({ ...value, [key]: v });

	return (
		<div className="space-y-2">
			<div className="flex items-center justify-between">
				<label className="text-xs font-medium text-neutral-400 uppercase tracking-wide">
					{label}
				</label>
				<div className="flex items-center gap-2">
					<div
						className="w-6 h-6 rounded border border-neutral-600"
						style={{
							background: `rgba(${value.red},${value.green},${value.blue},${value.alpha / 255})`,
						}}
					/>
					<input
						type="color"
						value={toHex(value)}
						onChange={(e) => {
							const rgb = hexToRgb(e.target.value);
							onChange({ ...value, ...rgb });
						}}
						className="w-6 h-6 cursor-pointer rounded border-0 bg-transparent p-0"
						title="Pick color"
					/>
				</div>
			</div>
			<div className="space-y-1.5 bg-neutral-900 rounded-lg p-3">
				<Channel label="R" value={value.red} onChange={set("red")} />
				<Channel label="G" value={value.green} onChange={set("green")} />
				<Channel label="B" value={value.blue} onChange={set("blue")} />
				<Channel label="A" value={value.alpha} onChange={set("alpha")} />
			</div>
			<div className="flex gap-1.5 flex-wrap">
				{PRESETS.map((p) => (
					<button
						key={p.label}
						onClick={() => onChange(p.color)}
						className="px-2 py-0.5 text-xs rounded bg-neutral-800 text-neutral-400 hover:bg-neutral-700 hover:text-neutral-200 border border-neutral-700 transition-colors"
					>
						{p.label}
					</button>
				))}
			</div>
		</div>
	);
}
