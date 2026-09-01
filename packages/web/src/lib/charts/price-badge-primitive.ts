// The last-price badge on the price axis: the current price, and beneath it
// the time until the forming bar closes, in one block — the shape a trader
// already reads on TradingView.
//
// Drawn by hand into the price-scale pane (`priceAxisPaneViews`) rather than
// returned as a `priceAxisViews` label, because an axis label is a single
// line of text. Two labels at different coordinates is not the same thing:
// it reads as two unrelated readings, and at any offset small enough to look
// related they collide.
//
// The series' own last-value label must be switched off wherever this is
// attached (`lastValueVisible: false`), or the price is drawn twice.
//
// The countdown ticks toward a deadline the *server* computed
// (`BarRangeResponse.next_bar_open_at`). Nothing re-asks the server every
// second, so once the deadline passes this rolls that same boundary forward
// by one whole bar step (`advanceDeadline`) — it never derives a boundary
// itself, because a bucket boundary depends on the series' anchor and a
// UTC-naive guess is wrong for a venue-anchored daily-or-coarser series.
import type {
	ISeriesApi,
	ISeriesPrimitive,
	IPrimitivePaneRenderer,
	IPrimitivePaneView,
	SeriesAttachedParameter,
	Time
} from 'lightweight-charts';
import type { CanvasRenderingTarget2D } from 'fancy-canvas';

import { advanceDeadline, formatCountdown } from './countdown';

/** Which way the forming bar is going — the badge's colour, and nothing else. */
export type PriceDirection = 'up' | 'down' | 'flat';

export interface PriceBadgeColors {
	/** Background while the bar is up. */
	up: string;
	/** Background while the bar is down. */
	down: string;
	/** Background when there is no live feed, so direction means nothing. */
	flat: string;
	/** Text on `up`/`down`. */
	onColor: string;
	/** Text on `flat`. */
	onFlat: string;
}

const FONT = "'IBM Plex Mono', monospace";
const PRICE_FONT_PX = 10;
const COUNTDOWN_FONT_PX = 9;
const PADDING_X = 6;
const PADDING_Y = 3;
const LINE_GAP = 2;

class BadgeRenderer implements IPrimitivePaneRenderer {
	constructor(private readonly primitive: LastPriceBadgePrimitive) {}

	draw(target: CanvasRenderingTarget2D): void {
		target.useMediaCoordinateSpace(({ context, mediaSize }) => {
			this.primitive.paint(context, mediaSize.width, mediaSize.height);
		});
	}
}

class BadgeView implements IPrimitivePaneView {
	constructor(private readonly primitive: LastPriceBadgePrimitive) {}

	renderer(): IPrimitivePaneRenderer {
		return new BadgeRenderer(this.primitive);
	}
}

export class LastPriceBadgePrimitive implements ISeriesPrimitive<Time> {
	private series: ISeriesApi<'Candlestick'> | undefined;
	private requestUpdate: (() => void) | undefined;
	private readonly view = new BadgeView(this);
	private ticker: ReturnType<typeof setInterval> | undefined;

	private price: number | null = null;
	private precision = 2;
	private direction: PriceDirection = 'flat';
	private deadlineMs: number | null = null;
	private stepMs: number | null = null;

	private colors: PriceBadgeColors = {
		up: '#7de0a3',
		down: '#e8836f',
		flat: 'rgba(255,255,255,0.14)',
		onColor: '#0a0a0c',
		onFlat: 'rgba(255,255,255,0.82)'
	};

	attached({ series, requestUpdate }: SeriesAttachedParameter<Time>): void {
		this.series = series as ISeriesApi<'Candlestick'>;
		this.requestUpdate = requestUpdate;
		// One timer per pane, not one per frame: the badge only changes once a
		// second, and repainting the axis on every animation frame to move a
		// clock would be work nobody can see.
		this.ticker = setInterval(() => {
			if (this.deadlineMs != null) this.requestUpdate?.();
		}, 1000);
	}

	detached(): void {
		if (this.ticker !== undefined) clearInterval(this.ticker);
		this.ticker = undefined;
		this.series = undefined;
		this.requestUpdate = undefined;
	}

	priceAxisPaneViews(): readonly IPrimitivePaneView[] {
		return [this.view];
	}

	/** The price to show, how many decimals it carries, and which way the
	 * forming bar is going. */
	setPrice(price: number | null, precision: number, direction: PriceDirection): void {
		this.price = price;
		this.precision = precision;
		this.direction = direction;
		this.requestUpdate?.();
	}

	/** Arms the countdown line. `nextBarOpenAtNanos` is the server's own
	 * boundary; `stepSeconds` is one bar of the pane's timeframe. */
	setDeadline(nextBarOpenAtNanos: number, stepSeconds: number): void {
		this.deadlineMs = nextBarOpenAtNanos / 1_000_000;
		this.stepMs = stepSeconds * 1000;
		this.requestUpdate?.();
	}

	/** Drops the countdown line, leaving a price-only badge — what a venue
	 * with no live feed gets, where a running clock would be a lie. */
	clearDeadline(): void {
		this.deadlineMs = null;
		this.stepMs = null;
		this.requestUpdate?.();
	}

	setColors(colors: PriceBadgeColors): void {
		this.colors = colors;
		this.requestUpdate?.();
	}

	private countdownText(): string | null {
		if (this.deadlineMs == null || this.stepMs == null) return null;
		const now = Date.now();
		if (this.deadlineMs <= now) {
			this.deadlineMs = advanceDeadline(this.deadlineMs, this.stepMs, now);
		}
		return formatCountdown(this.deadlineMs - now);
	}

	paint(context: CanvasRenderingContext2D, width: number, height: number): void {
		if (this.price == null || !this.series) return;
		const y = this.series.priceToCoordinate(this.price);
		if (y == null) return;

		const priceText = this.price.toFixed(this.precision);
		const countdown = this.countdownText();
		const background =
			this.direction === 'up'
				? this.colors.up
				: this.direction === 'down'
					? this.colors.down
					: this.colors.flat;
		const text = this.direction === 'flat' ? this.colors.onFlat : this.colors.onColor;

		context.save();
		context.textBaseline = 'middle';
		context.font = `${PRICE_FONT_PX}px ${FONT}`;
		const priceWidth = context.measureText(priceText).width;
		context.font = `${COUNTDOWN_FONT_PX}px ${FONT}`;
		const countdownWidth = countdown ? context.measureText(countdown).width : 0;

		const boxWidth = Math.min(
			width,
			Math.max(priceWidth, countdownWidth) + PADDING_X * 2
		);
		const lines = countdown ? 2 : 1;
		const boxHeight =
			PADDING_Y * 2 + PRICE_FONT_PX + (countdown ? LINE_GAP + COUNTDOWN_FONT_PX : 0);
		// Anchored so the price line — the reading the badge is *about* — stays
		// level with the price itself; the countdown hangs below it.
		const top = Math.max(
			0,
			Math.min(height - boxHeight, y - PADDING_Y - PRICE_FONT_PX / 2)
		);

		context.fillStyle = background;
		context.fillRect(0, top, boxWidth, boxHeight);

		context.fillStyle = text;
		context.font = `${PRICE_FONT_PX}px ${FONT}`;
		context.fillText(priceText, PADDING_X, top + PADDING_Y + PRICE_FONT_PX / 2);
		if (countdown) {
			context.font = `${COUNTDOWN_FONT_PX}px ${FONT}`;
			context.fillText(
				countdown,
				PADDING_X,
				top + PADDING_Y + PRICE_FONT_PX + LINE_GAP + COUNTDOWN_FONT_PX / 2
			);
		}
		context.restore();
	}
}
