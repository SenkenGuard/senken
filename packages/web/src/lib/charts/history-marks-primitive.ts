// Statements about *places* in the data, drawn where those places are.
//
// A pane makes two kinds of statement, and they do not belong in the same
// piece of chrome. "Loading earlier history" and "that request failed" are
// about something happening now: they have no position, they come and go, and
// a floating notice is right for them. "This is the earliest bar the venue
// has" and "the data is missing between here and here" are about a *location
// on the time axis* — they are permanent while the window holds, and a reader
// needs to see *where*, not merely *that*.
//
// Rendering the second kind as a centred badge over the plot was wrong twice:
// it covered the candles it was describing, and it threw away the only
// information that mattered. These are drawn on the chart instead, at the
// times they are about, so they pan and zoom with the bars.
//
// Never widens the price axis (`autoscaleInfo` returns `null`): a marker is
// not data, and it must not drag the visible range.
import type {
	IChartApi,
	IPrimitivePaneRenderer,
	IPrimitivePaneView,
	ISeriesApi,
	ISeriesPrimitive,
	SeriesAttachedParameter,
	Time
} from 'lightweight-charts';
import type { CanvasRenderingTarget2D } from 'fancy-canvas';

/** A range the server reported as not resolvable, in nanoseconds. */
export interface MarkedGap {
	from: number;
	to: number;
}

export interface HistoryMarkColors {
	/** The rule and its label. */
	line: string;
	text: string;
	/** Fill for a gap band. */
	gap: string;
}

const FONT = "9px 'IBM Plex Mono', monospace";
const LABEL_PAD_X = 5;
const LABEL_PAD_Y = 3;

class MarksRenderer implements IPrimitivePaneRenderer {
	constructor(private readonly primitive: HistoryMarksPrimitive) {}

	draw(target: CanvasRenderingTarget2D): void {
		target.useMediaCoordinateSpace(({ context, mediaSize }) => {
			this.primitive.paint(context, mediaSize.width, mediaSize.height);
		});
	}
}

class MarksPaneView implements IPrimitivePaneView {
	constructor(private readonly primitive: HistoryMarksPrimitive) {}

	renderer(): IPrimitivePaneRenderer {
		return new MarksRenderer(this.primitive);
	}
}

export class HistoryMarksPrimitive implements ISeriesPrimitive<Time> {
	private chart: IChartApi | undefined;
	private requestUpdate: (() => void) | undefined;
	private readonly view = new MarksPaneView(this);

	/** The earliest loaded bar, in seconds, when the venue has nothing older.
	 * `null` while more history may still exist — the mark is a claim, and it
	 * is only drawn once the claim has been earned. */
	private startOfHistorySeconds: number | null = null;
	private gaps: MarkedGap[] = [];
	/** The pane's bar grid, for placing a time that is not itself a plotted
	 * bar. A gap's edges never are, by definition — that is what makes it a
	 * gap — so `timeToCoordinate` returns `null` for both of them and only a
	 * logical index can be turned into an `x`. */
	private firstBarSeconds: number | undefined;
	private stepSeconds: number | undefined;

	private colors: HistoryMarkColors = {
		line: 'rgba(255,255,255,0.28)',
		text: 'rgba(255,255,255,0.55)',
		gap: 'rgba(255,255,255,0.05)'
	};

	attached({ chart, requestUpdate }: SeriesAttachedParameter<Time>): void {
		this.chart = chart as IChartApi;
		this.requestUpdate = requestUpdate;
	}

	detached(): void {
		this.chart = undefined;
		this.requestUpdate = undefined;
	}

	autoscaleInfo(): null {
		return null;
	}

	paneViews(): readonly IPrimitivePaneView[] {
		return [this.view];
	}

	setGrid(firstBarSeconds: number | undefined, stepSeconds: number | undefined): void {
		this.firstBarSeconds = firstBarSeconds;
		this.stepSeconds = stepSeconds;
		this.requestUpdate?.();
	}

	setStartOfHistory(seconds: number | null): void {
		this.startOfHistorySeconds = seconds;
		this.requestUpdate?.();
	}

	setGaps(gaps: MarkedGap[]): void {
		this.gaps = gaps;
		this.requestUpdate?.();
	}

	setColors(colors: HistoryMarkColors): void {
		this.colors = colors;
		this.requestUpdate?.();
	}

	/** Seconds to an `x`, for a time that may or may not be a plotted bar. */
	private xFor(seconds: number): number | null {
		const timeScale = this.chart?.timeScale();
		if (!timeScale) return null;
		const onBar = timeScale.timeToCoordinate(seconds as unknown as Time);
		if (onBar != null) return onBar;
		if (this.firstBarSeconds == null || !this.stepSeconds) return null;
		const logical = (seconds - this.firstBarSeconds) / this.stepSeconds;
		const coordinate = timeScale.logicalToCoordinate(
			logical as unknown as Parameters<typeof timeScale.logicalToCoordinate>[0]
		);
		return coordinate ?? null;
	}

	private paintLabel(
		context: CanvasRenderingContext2D,
		text: string,
		x: number,
		y: number,
		width: number
	): void {
		context.font = FONT;
		const textWidth = context.measureText(text).width;
		const boxWidth = textWidth + LABEL_PAD_X * 2;
		// Flips to the other side of the rule rather than running off the
		// pane: a label clipped at the edge is a label nobody can read.
		const left = x + 6 + boxWidth > width ? x - 6 - boxWidth : x + 6;
		context.fillStyle = this.colors.text;
		context.textBaseline = 'top';
		context.fillText(text, left + LABEL_PAD_X, y + LABEL_PAD_Y);
	}

	paint(context: CanvasRenderingContext2D, width: number, height: number): void {
		context.save();

		for (const gap of this.gaps) {
			const from = this.xFor(Math.floor(gap.from / 1_000_000_000));
			const to = this.xFor(Math.floor(gap.to / 1_000_000_000));
			if (from == null || to == null || to <= from) continue;
			context.fillStyle = this.colors.gap;
			context.fillRect(from, 0, to - from, height);
			context.strokeStyle = this.colors.line;
			context.setLineDash([3, 3]);
			context.lineWidth = 1;
			for (const edge of [from, to]) {
				context.beginPath();
				context.moveTo(edge + 0.5, 0);
				context.lineTo(edge + 0.5, height);
				context.stroke();
			}
			context.setLineDash([]);
			if (to - from > 60) this.paintLabel(context, 'NO DATA', from, 6, width);
		}

		if (this.startOfHistorySeconds != null) {
			const x = this.xFor(this.startOfHistorySeconds);
			if (x != null) {
				context.strokeStyle = this.colors.line;
				context.setLineDash([2, 3]);
				context.lineWidth = 1;
				context.beginPath();
				context.moveTo(x + 0.5, 0);
				context.lineTo(x + 0.5, height);
				context.stroke();
				context.setLineDash([]);
				this.paintLabel(context, 'START OF HISTORY', x, 6, width);
			}
		}

		context.restore();
	}
}
