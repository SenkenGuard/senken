// The client half of the dynamic settings form: turning an adapter's
// declared schema into form state, and form state back into a submission.
//
// # This is a courtesy, not a check
//
// Everything here mirrors `crates/trade/src/settings.rs`'s validation so
// the person typing sees a problem immediately instead of after a round
// trip. **The server validates every submission again regardless**, which
// is what actually holds — a client's copy of a schema can be stale, and a
// request need not have come from this client at all.
//
// # Secrets
//
// A stored credential never reaches the browser: the server serialises it
// as `null` and reports separately, in `secrets_set`, whether one exists.
// So a secret field starts empty, shows "configured" when one is on file,
// and is omitted from the submission when left blank — which the server
// reads as "keep what is stored".

import type {
	AdapterActionDto,
	SettingFieldDto,
	SettingsInputDto,
	SettingsSchemaDto,
	SettingsValuesDto
} from '$lib/api/types';
import { formatScaled, parseScaled } from './scaled';

/** One field's value while it is being edited. Text for everything but a
 * toggle, because that is what an `<input>` holds. */
export type FormState = Record<string, string | boolean>;

/** Per-field messages, keyed the same way as `FormState`. */
export type FormErrors = Record<string, string>;

/** The value a field starts at when nothing is stored for it. */
function defaultOf(field: SettingFieldDto): string | boolean {
	switch (field.type) {
		case 'toggle':
			return field.default;
		case 'number':
			return field.default === undefined ? '' : String(field.default);
		case 'decimal':
			return field.default === undefined
				? ''
				: formatScaled({ scale: field.scale, value: String(field.default) });
		case 'choice':
			return field.default ?? field.options[0]?.value ?? '';
		case 'secret':
			// Never prefilled: the stored credential is not in the document
			// this form was rendered from, and a placeholder that looked
			// like one would be submitted back as a literal value.
			return '';
		default:
			return field.default ?? '';
	}
}

/** Initial form state for `schema`, taking each field from `stored` when it
 * has a value there and from the field's own default otherwise. */
export function initialFormState(
	schema: SettingsSchemaDto,
	stored: SettingsValuesDto = {}
): FormState {
	const state: FormState = {};
	for (const field of schema.fields) {
		const value = stored[field.key];
		if (value === undefined || value === null) {
			state[field.key] = defaultOf(field);
			continue;
		}
		if (field.type === 'toggle') {
			state[field.key] = typeof value === 'boolean' ? value : defaultOf(field);
		} else if (field.type === 'decimal' && typeof value === 'object') {
			// A stored decimal arrives as `{ scale, value }`.
			state[field.key] = formatScaled(value as never);
		} else {
			state[field.key] = String(value);
		}
	}
	return state;
}

/** Checks one field, returning a message or `null`. Mirrors the server's
 * own rules; see this module's header for why it is not the real check. */
export function validateField(field: SettingFieldDto, raw: string | boolean): string | null {
	if (field.type === 'toggle') return null;

	const text = typeof raw === 'string' ? raw.trim() : '';
	if (text === '') {
		// A required secret already on file may be left blank — the server
		// keeps the stored one. Every other required field must have a
		// value or a default.
		if (field.required && field.type !== 'secret') return `${field.label} is required`;
		return null;
	}

	switch (field.type) {
		case 'number': {
			if (!/^-?\d+$/.test(text)) return `${field.label} must be a whole number`;
			const value = Number(text);
			if (value < field.min || value > field.max) {
				return `${field.label} must be between ${field.min} and ${field.max}`;
			}
			return null;
		}
		case 'decimal': {
			const parsed = parseScaled(text, field.scale);
			if (!parsed) {
				return field.scale === 0
					? `${field.label} must be a whole number`
					: `${field.label} takes at most ${field.scale} decimal places`;
			}
			const min = formatScaled({ scale: field.scale, value: String(field.min) });
			const max = formatScaled({ scale: field.scale, value: String(field.max) });
			// Comparing as numbers is safe for a *bound check* against
			// limits a schema author chose, even though formatting a value
			// that way never is.
			const value = Number(text);
			if (value < Number(min) || value > Number(max)) {
				return `${field.label} must be between ${min} and ${max}`;
			}
			return null;
		}
		case 'choice':
			return field.options.some((option) => option.value === text)
				? null
				: `${field.label} must be one of the listed options`;
		case 'text':
			return text.length > field.max_len
				? `${field.label} must be at most ${field.max_len} characters`
				: null;
		default:
			return null;
	}
}

/** Every field's message, for the fields that have one. */
export function validateForm(schema: SettingsSchemaDto, state: FormState): FormErrors {
	const errors: FormErrors = {};
	for (const field of schema.fields) {
		const message = validateField(field, state[field.key] ?? '');
		if (message) errors[field.key] = message;
	}
	return errors;
}

/** Turns form state into the submission body.
 *
 * A blank secret is **omitted**, not sent as an empty string: absent means
 * "keep the stored credential", while `""` would read as an attempt to set
 * one. Every other blank optional field is omitted too, so the server
 * applies its own default rather than this client guessing at it. */
export function toSubmission(schema: SettingsSchemaDto, state: FormState): SettingsInputDto {
	const input: SettingsInputDto = {};
	for (const field of schema.fields) {
		const raw = state[field.key];
		if (field.type === 'toggle') {
			input[field.key] = typeof raw === 'boolean' ? raw : false;
			continue;
		}
		const text = typeof raw === 'string' ? raw.trim() : '';
		if (text === '') continue;
		input[field.key] = field.type === 'number' ? Number(text) : text;
	}
	return input;
}

/** Whether a secret field should read "configured" rather than empty. */
export function secretIsSet(field: SettingFieldDto, secretsSet: Record<string, boolean>): boolean {
	return field.type === 'secret' && secretsSet[field.key] === true;
}

/** The placeholder a secret input shows: what it says depends on whether a
 * credential is already on file, because "leave blank to keep" is only true
 * when there is something to keep. */
export function secretPlaceholder(
	field: SettingFieldDto,
	secretsSet: Record<string, boolean>
): string {
	if (field.type !== 'secret') return '';
	return secretIsSet(field, secretsSet)
		? 'Configured — leave blank to keep'
		: (field.placeholder ?? '');
}

/** Whether an action needs a form at all. */
export function actionHasForm(action: AdapterActionDto): boolean {
	return action.form.fields.length > 0;
}
