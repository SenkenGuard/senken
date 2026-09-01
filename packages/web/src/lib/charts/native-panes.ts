import type { UTCTimestamp } from 'lightweight-charts';

export interface IndicatorPoint {
	time: number;
	value: number;
}

/** Converts indicator values without manufacturing warm-up whitespace. */
export function nativePaneData(
	points: IndicatorPoint[],
	divisor: number
): { time: UTCTimestamp; value: number }[] {
	return points.map((point) => ({
		time: point.time as UTCTimestamp,
		value: point.value / divisor
	}));
}
