import { describe, expect, test } from 'bun:test';
import { isLoopbackHost, isSecureUrl } from './security';

describe('isLoopbackHost', () => {
	test.each(['localhost', '127.0.0.1', '::1', '[::1]', 'foo.localhost'])('%s is loopback', (host) => {
		expect(isLoopbackHost(host)).toBe(true);
	});

	test.each(['trading.example.com', '192.168.1.50', '0.0.0.0'])('%s is not loopback', (host) => {
		expect(isLoopbackHost(host)).toBe(false);
	});
});

describe('isSecureUrl', () => {
	test('loopback http is secure (OS already authenticated the person at the machine)', () => {
		expect(isSecureUrl('http://127.0.0.1:4206')).toBe(true);
		expect(isSecureUrl('http://localhost:5173')).toBe(true);
	});

	test('non-loopback https is secure', () => {
		expect(isSecureUrl('https://trading.example.com')).toBe(true);
	});

	test('non-loopback http is INSECURE — the case B15 exists to warn about', () => {
		expect(isSecureUrl('http://trading.example.com')).toBe(false);
		expect(isSecureUrl('http://192.168.1.50:4206')).toBe(false);
	});

	test('an unparsable URL is treated as insecure, not silently passed', () => {
		expect(isSecureUrl('not a url')).toBe(false);
	});
});
