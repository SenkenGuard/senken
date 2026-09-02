// Renders the real, compiled form through Svelte's own server renderer and
// reads the actual markup, rather than asserting on the logic that is
// supposed to produce it. Tailwind reports nothing for a class that does
// not exist, and a control that was never emitted looks exactly like one
// that was — both are only visible in the output.
import { describe, expect, test } from 'bun:test';
import { render } from 'svelte/server';
import type { SettingsSchemaDto } from '$lib/api/types';
import SchemaForm from './schema-form.svelte';

const schema: SettingsSchemaDto = {
	fields: [
		{
			key: 'api_key',
			label: 'API key',
			required: true,
			type: 'secret',
			placeholder: 'sk-…'
		},
		{
			key: 'starting_balance',
			label: 'Starting balance',
			required: true,
			help: 'What the account opens with.',
			type: 'decimal',
			scale: 2,
			default: 10000000,
			min: 0,
			max: 100000000000,
			unit: 'USD'
		},
		{
			key: 'currency',
			label: 'Currency',
			required: true,
			type: 'choice',
			default: 'USD',
			options: [
				{ value: 'USD', label: 'US dollar' },
				{ value: 'EUR', label: 'Euro' }
			]
		},
		{ key: 'allow_short', label: 'Allow shorting', required: true, type: 'toggle', default: true },
		{ key: 'nickname', label: 'Nickname', required: false, type: 'text', max_len: 16 }
	]
};

const state = {
	api_key: '',
	starting_balance: '100000.00',
	currency: 'USD',
	allow_short: true,
	nickname: ''
};

describe('schema form (real DOM output)', () => {
	test('emits one labelled control per declared field, tagged with its kind', () => {
		const { body } = render(SchemaForm, { props: { schema, state } });
		for (const field of schema.fields) {
			expect(body).toContain(`data-field="${field.key}"`);
			expect(body).toContain(`data-field-type="${field.type}"`);
			expect(body).toContain(field.label);
		}
	});

	test('a secret field renders as a password input, never as readable text', () => {
		const { body } = render(SchemaForm, { props: { schema, state } });
		expect(body).toContain('type="password"');
		expect(body).toContain('id="field-api_key"');
	});

	test('a secret already on file says so instead of looking empty', () => {
		const { body } = render(SchemaForm, {
			props: { schema, state, secretsSet: { api_key: true } }
		});
		expect(body).toContain('Configured');
	});

	test('a secret not yet set shows the adapter placeholder rather than that copy', () => {
		const { body } = render(SchemaForm, {
			props: { schema, state, secretsSet: { api_key: false } }
		});
		expect(body).not.toContain('Configured');
		expect(body).toContain('sk-…');
	});

	test('a choice field emits an option per declared option, with its label', () => {
		const { body } = render(SchemaForm, { props: { schema, state } });
		expect(body).toContain('US dollar');
		expect(body).toContain('Euro');
	});

	test("a numeric field shows the adapter's own unit", () => {
		const { body } = render(SchemaForm, { props: { schema, state } });
		expect(body).toContain('USD');
	});

	test('an optional field is marked optional and a required one is not', () => {
		const { body } = render(SchemaForm, { props: { schema, state } });
		expect(body).toContain('OPTIONAL');
		// One field is optional; the marker must not have been emitted for
		// the other four.
		expect(body.split('OPTIONAL').length - 1).toBe(1);
	});

	test('help text shows when there is no error, and the error replaces it', () => {
		const clean = render(SchemaForm, { props: { schema, state } });
		expect(clean.body).toContain('What the account opens with.');
		expect(clean.body).not.toContain('data-field-error');

		const failed = render(SchemaForm, {
			props: { schema, state, errors: { starting_balance: 'Too small' } }
		});
		expect(failed.body).toContain('data-field-error');
		expect(failed.body).toContain('Too small');
		expect(failed.body).not.toContain('What the account opens with.');
	});

	test('a toggle renders as a switch that reports its own state in the DOM', () => {
		const on = render(SchemaForm, { props: { schema, state } });
		expect(on.body).toContain('role="switch"');
		expect(on.body).toContain('aria-checked="true"');

		const off = render(SchemaForm, {
			props: { schema, state: { ...state, allow_short: false } }
		});
		expect(off.body).toContain('aria-checked="false"');
	});

	test('an adapter with no settings says so rather than rendering an empty box', () => {
		const { body } = render(SchemaForm, { props: { schema: { fields: [] }, state: {} } });
		expect(body).toContain('needs no settings');
	});
});
