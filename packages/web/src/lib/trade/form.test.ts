import { describe, expect, test } from 'bun:test';
import type { SettingsSchemaDto } from '$lib/api/types';
import {
	initialFormState,
	secretPlaceholder,
	toSubmission,
	validateField,
	validateForm
} from './form';

const schema: SettingsSchemaDto = {
	fields: [
		{ key: 'api_key', label: 'API key', required: true, type: 'secret', placeholder: 'sk-…' },
		{
			key: 'starting_balance',
			label: 'Starting balance',
			required: true,
			type: 'decimal',
			scale: 2,
			default: 10000000,
			min: 0,
			max: 100000000000
		},
		{
			key: 'leverage',
			label: 'Leverage',
			required: true,
			type: 'number',
			default: 1,
			min: 1,
			max: 125,
			unit: 'x'
		},
		{
			key: 'currency',
			label: 'Currency',
			required: true,
			type: 'choice',
			default: 'USD',
			options: [
				{ value: 'USD', label: 'USD' },
				{ value: 'EUR', label: 'EUR' }
			]
		},
		{ key: 'allow_short', label: 'Allow shorting', required: true, type: 'toggle', default: true },
		{ key: 'nickname', label: 'Nickname', required: false, type: 'text', max_len: 8 }
	]
};

describe('initialFormState', () => {
	test('renders a decimal default at its own scale rather than as a raw integer', () => {
		// The schema carries 10000000 at scale 2; a form showing that
		// number is off by a factor of a hundred.
		expect(initialFormState(schema).starting_balance).toBe('100000.00');
	});

	test('takes the rest of each field from its declared default', () => {
		const state = initialFormState(schema);
		expect(state.leverage).toBe('1');
		expect(state.currency).toBe('USD');
		expect(state.allow_short).toBe(true);
	});

	test('never prefills a secret, even when one is stored', () => {
		// The stored credential is not in the document this form was
		// rendered from; anything shown here would be submitted back as a
		// literal value.
		const state = initialFormState(schema, { api_key: null, leverage: 20 });
		expect(state.api_key).toBe('');
		expect(state.leverage).toBe('20');
	});
});

describe('validateField', () => {
	test('accepts a decimal at exactly the declared scale', () => {
		expect(validateField(schema.fields[1], '250.00')).toBeNull();
	});

	test('refuses a decimal finer than the scale rather than rounding it', () => {
		expect(validateField(schema.fields[1], '1.005')).toContain('2 decimal places');
	});

	test('refuses a number outside its bounds, naming them', () => {
		expect(validateField(schema.fields[2], '500')).toContain('between 1 and 125');
	});

	test('refuses a choice that is not an option', () => {
		expect(validateField(schema.fields[3], 'IDR')).toContain('one of the listed options');
	});

	test('refuses text longer than the field allows', () => {
		expect(validateField(schema.fields[5], 'far too long')).toContain('at most 8');
	});

	test('lets a required secret be left blank, because the stored one is kept', () => {
		expect(validateField(schema.fields[0], '')).toBeNull();
	});

	test('refuses any other required field left blank', () => {
		expect(validateField(schema.fields[1], '')).toContain('required');
	});

	test('accepts a blank optional field', () => {
		expect(validateField(schema.fields[5], '')).toBeNull();
	});
});

describe('validateForm', () => {
	test('reports nothing for the schema defaults', () => {
		expect(validateForm(schema, initialFormState(schema))).toEqual({});
	});

	test('reports one message per failing field, keyed by field', () => {
		const errors = validateForm(schema, {
			...initialFormState(schema),
			leverage: '500',
			currency: 'IDR'
		});
		expect(Object.keys(errors).sort()).toEqual(['currency', 'leverage']);
	});
});

describe('toSubmission', () => {
	test('omits a blank secret so the stored credential is kept', () => {
		// Sending `""` would read as an attempt to set an empty one.
		const body = toSubmission(schema, initialFormState(schema));
		expect('api_key' in body).toBe(false);
	});

	test('sends a secret that was actually typed', () => {
		const state = { ...initialFormState(schema), api_key: 'sk-live-1' };
		expect(toSubmission(schema, state).api_key).toBe('sk-live-1');
	});

	test('sends a decimal as the text the user typed, never as a number', () => {
		// A JSON number would put a money value through a double on the
		// way out.
		const state = { ...initialFormState(schema), starting_balance: '0.10' };
		expect(toSubmission(schema, state).starting_balance).toBe('0.10');
	});

	test('sends a whole number as a number and a toggle as a boolean', () => {
		const body = toSubmission(schema, initialFormState(schema));
		expect(body.leverage).toBe(1);
		expect(body.allow_short).toBe(true);
	});

	test('omits a blank optional field so the server applies its own default', () => {
		const body = toSubmission(schema, initialFormState(schema));
		expect('nickname' in body).toBe(false);
	});
});

describe('secretPlaceholder', () => {
	test('says a credential can be kept only when there is one to keep', () => {
		expect(secretPlaceholder(schema.fields[0], { api_key: true })).toContain('leave blank to keep');
		expect(secretPlaceholder(schema.fields[0], { api_key: false })).toBe('sk-…');
	});
});
