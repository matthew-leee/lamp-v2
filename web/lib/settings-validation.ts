/**
 * Save-time format validation for admin settings (findings G23).
 *
 * Pure, dependency-free rules shared by the `/admin/api/settings/[key]` route
 * and the unit tests. An EMPTY value always passes: it means "clear this
 * setting" (the `off` action / `revert` path); only NON-EMPTY values are
 * format-checked, so a malformed analytics ID, phone number or URL can never
 * reach the live storefront configuration.
 */

export interface SettingValueRule {
  hint: string;
  /** Format check for ONE non-empty, trimmed value. */
  isValid: (value: string) => boolean;
}

/**
 * Human phone formatting (spaces, dashes) is free-form; the DIGITS are not:
 * after stripping spaces/dashes the value must be 7–15 actual digits with an
 * optional `+` prefix (E.164-ish). Parentheses, letters and 6- or 16-digit
 * strings never pass.
 */
function isValidWhatsAppNumber(value: string): boolean {
  return /^\+?[0-9]{7,15}$/.test(value.replace(/[\s-]/g, ""));
}

/**
 * A real, absolute web URL: `new URL` must parse the value AND it must speak
 * `http:`/`https:` with a non-empty hostname — `javascript:`, `data:`,
 * schemeless text and host-less strings never pass.
 */
function isValidSiteUrl(value: string): boolean {
  try {
    const url = new URL(value);
    return (
      (url.protocol === "http:" || url.protocol === "https:") &&
      url.hostname !== ""
    );
  } catch {
    return false;
  }
}

export const SETTING_VALUE_RULES: Record<string, SettingValueRule> = {
  "analytics.ga_measurement_id": {
    hint: "must be a GA4 Measurement ID like G-XXXXXXXXXX",
    isValid: (value) => /^G-[A-Z0-9]{4,12}$/.test(value),
  },
  "contact.whatsapp": {
    hint: "must contain 7–15 digits (spaces and dashes allowed), optionally starting with +",
    isValid: isValidWhatsAppNumber,
  },
  "site.url": {
    hint: "must be an absolute http(s) URL like https://shop.example.com",
    isValid: isValidSiteUrl,
  },
};

export type SettingValueVerdict = { ok: true } | { ok: false; error: string };

export function validateSettingValue(
  key: string,
  value: string,
): SettingValueVerdict {
  const rule = SETTING_VALUE_RULES[key];
  if (!rule) return { ok: true };
  const trimmed = value.trim();
  // Empty = "clear the override"; never block reverting to the default.
  if (trimmed === "") return { ok: true };
  if (!rule.isValid(trimmed)) {
    return {
      ok: false,
      error: `Invalid format for ${key}: value ${rule.hint}.`,
    };
  }
  return { ok: true };
}
