// Proves the two checks `plugin-widget-frame.svelte` applies to every
// inbound `postMessage` before trusting a single field of it: the message
// must look like it came from *this* widget's own sandboxed iframe
// (`isFromSandboxedWidget`), and it must be *shaped* like a message this
// host actually routes (`isWidgetToHostMessage`). Each accepting test is
// paired with a mutation of exactly one field that must flip the result to
// `false` — proving the check actually discriminates, not merely that a
// hand-picked good example passes and a hand-picked bad one fails.
import { describe, expect, test } from 'bun:test';
import { isFromSandboxedWidget, isWidgetToHostMessage } from './widget-message-protocol';

function validMessage(overrides: Record<string, unknown> = {}): Record<string, unknown> {
	return {
		channel: 'senken.widget',
		v: 1,
		id: 'req-1',
		method: 'ready',
		...overrides
	};
}

describe('isWidgetToHostMessage', () => {
	test('accepts a well-formed message for each routed method', () => {
		for (const method of ['ready', 'config.get', 'config.patch']) {
			expect(isWidgetToHostMessage(validMessage({ method }))).toBe(true);
		}
	});

	test('accepts an unrecognized extra field (forward compatibility)', () => {
		expect(isWidgetToHostMessage(validMessage({ somethingNew: 42 }))).toBe(true);
	});

	// --- mutate one field at a time and prove each one alone is decisive ---

	test('rejects a non-object payload entirely', () => {
		expect(isWidgetToHostMessage('senken.widget')).toBe(false);
		expect(isWidgetToHostMessage(null)).toBe(false);
		expect(isWidgetToHostMessage(undefined)).toBe(false);
		expect(isWidgetToHostMessage(42)).toBe(false);
	});

	test('rejects the wrong channel name', () => {
		expect(isWidgetToHostMessage(validMessage({ channel: 'not-senken-widget' }))).toBe(false);
	});

	test('rejects a missing channel', () => {
		const message = validMessage();
		delete message.channel;
		expect(isWidgetToHostMessage(message)).toBe(false);
	});

	test('rejects the wrong protocol version, even the numerically-equal string', () => {
		expect(isWidgetToHostMessage(validMessage({ v: 2 }))).toBe(false);
		expect(isWidgetToHostMessage(validMessage({ v: '1' }))).toBe(false);
	});

	test('rejects an empty or oversized id', () => {
		expect(isWidgetToHostMessage(validMessage({ id: '' }))).toBe(false);
		expect(isWidgetToHostMessage(validMessage({ id: 'x'.repeat(129) }))).toBe(false);
		// The boundary itself must still be accepted.
		expect(isWidgetToHostMessage(validMessage({ id: 'x'.repeat(128) }))).toBe(true);
	});

	test('rejects a non-string id', () => {
		expect(isWidgetToHostMessage(validMessage({ id: 1 }))).toBe(false);
	});

	test('rejects a method this host does not route', () => {
		expect(isWidgetToHostMessage(validMessage({ method: 'topic.subscribe' }))).toBe(false);
		expect(isWidgetToHostMessage(validMessage({ method: 'command.call' }))).toBe(false);
	});

	test('rejects a method that is merely a prefix or suffix of a real one', () => {
		expect(isWidgetToHostMessage(validMessage({ method: 'config' }))).toBe(false);
		expect(isWidgetToHostMessage(validMessage({ method: 'config.get.extra' }))).toBe(false);
	});
});

describe('isFromSandboxedWidget', () => {
	// A `Window` cannot be constructed in this test environment (no real
	// DOM), so two distinct plain objects stand in for "this iframe's own
	// contentWindow" and "some other window" — `isFromSandboxedWidget` only
	// ever compares `event.source` against `iframe.contentWindow` by
	// identity, so a stand-in object is exactly as good a test double as a
	// real `Window` would be for this property.
	const ownWindow = {} as Window;
	const otherWindow = {} as Window;
	const iframe = { contentWindow: ownWindow } as unknown as HTMLIFrameElement;

	test('accepts a message from the iframe itself with an opaque origin', () => {
		expect(isFromSandboxedWidget({ source: ownWindow, origin: 'null' }, iframe)).toBe(true);
	});

	test('rejects a message whose source is not this iframe', () => {
		expect(isFromSandboxedWidget({ source: otherWindow, origin: 'null' }, iframe)).toBe(false);
	});

	test('rejects a message with a real (non-opaque) origin, even from the right source', () => {
		// This is the property the platform's design record calls out by
		// name: a sandboxed iframe's origin is always the literal string
		// "null" — a message reporting any other origin, including one
		// that looks like this exact server's own origin, cannot actually
		// have come from the sandboxed document and must be rejected.
		expect(
			isFromSandboxedWidget({ source: ownWindow, origin: 'https://example.com' }, iframe)
		).toBe(false);
		expect(
			isFromSandboxedWidget({ source: ownWindow, origin: 'http://localhost:5173' }, iframe)
		).toBe(false);
	});

	test('rejects when there is no iframe to compare against yet', () => {
		expect(isFromSandboxedWidget({ source: ownWindow, origin: 'null' }, undefined)).toBe(false);
	});
});
