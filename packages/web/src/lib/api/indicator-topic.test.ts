import { describe, expect, test } from 'bun:test';
import { indicatorTopic } from './indicator-topic';

describe('indicatorTopic', () => {
	test('matches crates/api/src/ws.rs IndicatorSubscribeRequest::topic() field-for-field', () => {
		// Mirrors that module's own `subscribing_to_a_live_indicator_streams_readings_as_ticks_arrive`
		// test fixture exactly, so a divergence here would also break the
		// wire contract, not just this file's own expectations.
		const topic = indicatorTopic({
			instrument: 'okx-spot:BTC-USDT',
			spec: '1m',
			indicator: 'Sma',
			params: '{"period":1}'
		});
		expect(topic).toBe('indicator:okx-spot:BTC-USDT:1m:Sma:{"period":1}');
	});

	test('is plain concatenation of the raw fields, not a canonicalized form', () => {
		// Two requests whose `params` differ only in key order produce two
		// different topics — the server does not canonicalize either, so this
		// module must not start doing so on its own and silently merging
		// sessions the server keeps apart.
		const a = indicatorTopic({ instrument: 'okx-spot:BTC-USDT', spec: '1h', indicator: 'Macd', params: '{"fast":12,"slow":26}' });
		const b = indicatorTopic({ instrument: 'okx-spot:BTC-USDT', spec: '1h', indicator: 'Macd', params: '{"slow":26,"fast":12}' });
		expect(a).not.toBe(b);
	});

	test('two different indicators on the same instrument/spec resolve to two different topics', () => {
		const sma = indicatorTopic({ instrument: 'okx-spot:BTC-USDT', spec: '1h', indicator: 'Sma', params: '{"period":20}' });
		const ema = indicatorTopic({ instrument: 'okx-spot:BTC-USDT', spec: '1h', indicator: 'Ema', params: '{"period":20}' });
		expect(sma).not.toBe(ema);
	});
});
