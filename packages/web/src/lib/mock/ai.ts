// Mock data for the AI panel. Ported from `aiMessages` / `aiPrompts` (lines 3199-3211).
//
// UI only: no `fetch`, no `/api`, no model call — this is
// a fixed transcript, not a live conversation.
//
// Deviation: the reference parametrizes these two messages by
// `symOf(s.symbol)` / `lastPrice(s.symbol)`, its single page-wide "current
// instrument". `AiPanel` is global chrome mounted once in `AppShell`
// (reachable from all three routes), and those routes each own their own
// notion of "current symbol" independently (or, on the dashboard and trade
// engine pages, none at all) rather than sharing the reference's one-page
// state — so there is no cross-route symbol to read here. Fixed on
// BTC/USDT, the terminal's first fixture instrument, rather than inventing
// a shared "current instrument" bus P5 was not asked to build.
//
// `./charts` is gone (the mock module the charts page used to
// read from — real data now, see `$lib/charts/`). This panel's own fixture
// ticker/level were only ever cosmetic copy-text parameters, never a real
// price feed, so they are inlined directly rather than reaching into the
// charts page's now-real data layer for a fixed transcript that was never
// going to be live either way.

export const AI_PROMPTS = ['SUMMARISE CHART', 'RISK CHECK', 'FIND SETUPS'];

export interface AiMessage {
	text: string;
	align: 'user' | 'ai';
}

const TICKER = 'BTCUSDT';
const LEVEL = '67,404.30';

export function buildAiMessages(): AiMessage[] {
	const ticker = TICKER;
	const level = LEVEL;
	return [
		{ align: 'user', text: `Forecast ${ticker} into the London close.` },
		{
			align: 'ai',
			text: `Spot inflow is positive three sessions running and funding is neutral. Bias stays long above ${level}; a loss of that level flips the read to range.`
		},
		{ align: 'user', text: 'Risk if CPI comes in hot?' },
		{ align: 'ai', text: 'Expect a first-move fade. Size to 0.3R and keep the stop outside the 1H ATR band.' }
	];
}
