import type { AppSettings } from "./types";

export interface DictationShortcutKeyEvent {
  code: string;
  key: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
}

const LETTER_KEY_CODES = Array.from(
  { length: 26 },
  (_, index) => `Key${String.fromCharCode(65 + index)}`,
);
const DIGIT_KEY_CODES = Array.from(
  { length: 10 },
  (_, index) => `Digit${index}`,
);
const FUNCTION_KEY_CODES = Array.from(
  { length: 24 },
  (_, index) => `F${index + 1}`,
);
const NAMED_KEY_CODES = [
  "Space",
  "Enter",
  "Tab",
  "Backspace",
  "Delete",
  "ArrowUp",
  "ArrowDown",
  "ArrowLeft",
  "ArrowRight",
  "Home",
  "End",
  "PageUp",
  "PageDown",
] as const;

export const SUPPORTED_DICTATION_KEY_CODES = [
  ...LETTER_KEY_CODES,
  ...DIGIT_KEY_CODES,
  ...FUNCTION_KEY_CODES,
  ...NAMED_KEY_CODES,
] as const;

const MODIFIER_ALIASES: Readonly<
  Record<string, "ctrl" | "alt" | "shift" | "cmd">
> = {
  ctrl: "ctrl",
  control: "ctrl",
  alt: "alt",
  option: "alt",
  shift: "shift",
  cmd: "cmd",
  command: "cmd",
  meta: "cmd",
  super: "cmd",
};

const MODIFIER_ORDER = ["ctrl", "alt", "shift", "cmd"] as const;
const PHYSICAL_MODIFIER_ALIASES: Readonly<Record<string, string>> = {
  leftctrl: "leftctrl",
  leftcontrol: "leftctrl",
  ctrlleft: "leftctrl",
  controlleft: "leftctrl",
  rightctrl: "rightctrl",
  rightcontrol: "rightctrl",
  ctrlright: "rightctrl",
  controlright: "rightctrl",
  leftalt: "leftalt",
  leftoption: "leftalt",
  altleft: "leftalt",
  optionleft: "leftalt",
  rightalt: "rightalt",
  rightoption: "rightalt",
  altright: "rightalt",
  optionright: "rightalt",
  leftshift: "leftshift",
  shiftleft: "leftshift",
  rightshift: "rightshift",
  shiftright: "rightshift",
  leftcmd: "leftcmd",
  leftcommand: "leftcmd",
  cmdleft: "leftcmd",
  commandleft: "leftcmd",
  leftmeta: "leftcmd",
  metaleft: "leftcmd",
  rightcmd: "rightcmd",
  rightcommand: "rightcmd",
  cmdright: "rightcmd",
  commandright: "rightcmd",
  rightmeta: "rightcmd",
  metaright: "rightcmd",
};
const NAMED_MAIN_KEYS = new Set([
  "space",
  "enter",
  "tab",
  "backspace",
  "delete",
  "arrowup",
  "arrowdown",
  "arrowleft",
  "arrowright",
  "home",
  "end",
  "pageup",
  "pagedown",
]);

export function normalizeDictationShortcut(raw: string): string {
  const parts = raw.split("+").map((part) => part.trim().toLowerCase());
  if (parts.length === 0 || parts.some((part) => part.length === 0)) {
    throw new Error("Dictation shortcut cannot be empty");
  }
  if (parts.some((part) => part === "fn" || part === "function")) {
    throw new Error("The fn key is not supported for dictation shortcuts");
  }
  if (parts.length === 1) {
    const physicalModifier = PHYSICAL_MODIFIER_ALIASES[parts[0] ?? ""];
    if (physicalModifier) {
      return physicalModifier;
    }
  }
  if (parts.some((part) => PHYSICAL_MODIFIER_ALIASES[part])) {
    throw new Error("A physical modifier shortcut must be used alone");
  }

  const modifiers = new Set<(typeof MODIFIER_ORDER)[number]>();
  const mainKeys: string[] = [];
  for (const part of parts) {
    const modifier = MODIFIER_ALIASES[part];
    if (modifier) {
      if (modifiers.has(modifier)) {
        throw new Error(`Duplicate shortcut modifier: ${modifier}`);
      }
      modifiers.add(modifier);
    } else {
      mainKeys.push(part);
    }
  }
  if (mainKeys.length === 0 && modifiers.size >= 2) {
    return MODIFIER_ORDER.filter((modifier) => modifiers.has(modifier)).join(
      "+",
    );
  }
  if (mainKeys.length !== 1) {
    throw new Error("Dictation shortcut must contain exactly one main key");
  }

  const mainKey = mainKeys[0] ?? "";
  const functionKey = /^f(?:[1-9]|1\d|2[0-4])$/.test(mainKey);
  const supportedMainKey =
    /^[a-z0-9]$/.test(mainKey) || functionKey || NAMED_MAIN_KEYS.has(mainKey);
  if (!supportedMainKey) {
    throw new Error(`Unsupported dictation shortcut key: ${mainKey}`);
  }
  return [
    ...MODIFIER_ORDER.filter((modifier) => modifiers.has(modifier)),
    mainKey,
  ].join("+");
}

export function dictationShortcutFromKeyboardEvent(
  event: DictationShortcutKeyEvent,
): string | null {
  const mainKey = mainKeyFromCode(event.code);
  const physicalModifier = physicalModifierFromCode(event.code);
  if (!mainKey && !physicalModifier) {
    return null;
  }
  const modifiers = [
    event.ctrlKey ? "ctrl" : null,
    event.altKey ? "alt" : null,
    event.shiftKey ? "shift" : null,
    event.metaKey ? "cmd" : null,
  ].filter((part): part is string => Boolean(part));
  if (!mainKey && modifiers.length < 2) {
    return physicalModifier;
  }
  const parts = [...modifiers, mainKey ?? null].filter((part): part is string =>
    Boolean(part),
  );
  try {
    return normalizeDictationShortcut(parts.join("+"));
  } catch {
    return null;
  }
}

export function formatDictationShortcut(shortcut: string): string {
  const labels: Readonly<Record<string, string>> = {
    ctrl: "Control",
    alt: "Option",
    shift: "Shift",
    cmd: "Command",
    leftctrl: "Left Control",
    rightctrl: "Right Control",
    leftalt: "Left Option",
    rightalt: "Right Option",
    leftshift: "Left Shift",
    rightshift: "Right Shift",
    leftcmd: "Left Command",
    rightcmd: "Right Command",
    space: "Space",
    enter: "Enter",
    tab: "Tab",
    backspace: "Backspace",
    delete: "Delete",
    arrowup: "Arrow Up",
    arrowdown: "Arrow Down",
    arrowleft: "Arrow Left",
    arrowright: "Arrow Right",
    home: "Home",
    end: "End",
    pageup: "Page Up",
    pagedown: "Page Down",
  };
  return normalizeDictationShortcut(shortcut)
    .split("+")
    .map((part) => labels[part] ?? part.toUpperCase())
    .join(" + ");
}

function isModifierShortcut(shortcut: string): boolean {
  return (
    /^(?:left|right)(?:ctrl|alt|shift|cmd)$/.test(shortcut) ||
    shortcut
      .split("+")
      .every((part) =>
        MODIFIER_ORDER.includes(part as (typeof MODIFIER_ORDER)[number]),
      )
  );
}

export function normalizeEnhancedPromptShortcut(shortcut: string): string {
  const normalized = normalizeDictationShortcut(shortcut);
  if (isModifierShortcut(normalized)) {
    throw new Error("Enhanced Prompt requires a shortcut with a main key");
  }
  return normalized;
}

export function dictationShortcutsOverlap(
  left: string,
  right: string,
): boolean {
  left = normalizeDictationShortcut(left);
  right = normalizeDictationShortcut(right);
  if (left === right) return true;
  const physical = /^(?:left|right)(?:ctrl|alt|shift|cmd)$/;
  if (physical.test(left) && physical.test(right)) return false;
  const logical = (part: string) => part.replace(/^(left|right)/, "");
  return [
    [left, right],
    [right, left],
  ].some(
    ([required, other]) =>
      isModifierShortcut(required) &&
      required
        .split("+")
        .map(logical)
        .every((part) => other.split("+").map(logical).includes(part)),
  );
}

export function validateDictationHotkeys(
  settings: Pick<
    AppSettings,
    | "dictation_enabled"
    | "dictation_shortcut"
    | "dictation_toggle_shortcut"
    | "enhance_prompt_enabled"
    | "enhance_prompt_shortcut"
  > & Partial<Pick<AppSettings, "cycle_microphone_shortcut">>,
): void {
  const hotkeys = settings.dictation_enabled
    ? [["Hold-to-dictate hotkey", settings.dictation_shortcut]] : [];
  if (settings.dictation_enabled && settings.dictation_toggle_shortcut) {
    hotkeys.push([
      "Toggle dictation hotkey",
      settings.dictation_toggle_shortcut,
    ]);
  }
  if (settings.dictation_enabled && settings.enhance_prompt_enabled) {
    normalizeEnhancedPromptShortcut(settings.enhance_prompt_shortcut);
    hotkeys.push([
      "Enhanced Prompt shortcut",
      settings.enhance_prompt_shortcut,
    ]);
  }
  const cycle = normalizeCycleMicrophoneShortcut(settings.cycle_microphone_shortcut ?? "");
  if (cycle) hotkeys.push(["Cycle Primary microphone shortcut", cycle]);
  for (const [index, [label, shortcut]] of hotkeys.entries()) {
    normalizeDictationShortcut(shortcut);
    for (const [otherLabel, other] of hotkeys.slice(0, index)) {
      if (dictationShortcutsOverlap(shortcut, other)) {
        throw new Error(
          `${label} overlaps ${otherLabel}. Choose a different key combination.`,
        );
      }
    }
  }
}

export function normalizeCycleMicrophoneShortcut(raw: string): string {
  if (!raw.trim()) return "";
  const value = normalizeDictationShortcut(raw);
  if (isModifierShortcut(value) || !value.includes("+")) {
    throw new Error("Cycle microphone requires a modifier and a main key, such as Ctrl+Shift+M");
  }
  return value;
}

function physicalModifierFromCode(code: string): string | null {
  const modifiers: Readonly<Record<string, string>> = {
    ControlLeft: "leftctrl",
    ControlRight: "rightctrl",
    AltLeft: "leftalt",
    AltRight: "rightalt",
    ShiftLeft: "leftshift",
    ShiftRight: "rightshift",
    MetaLeft: "leftcmd",
    MetaRight: "rightcmd",
  };
  return modifiers[code] ?? null;
}

function mainKeyFromCode(code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) {
    return code.slice(3).toLowerCase();
  }
  if (/^Digit\d$/.test(code)) {
    return code.slice(5);
  }
  if (/^F(?:[1-9]|1\d|2[0-4])$/.test(code)) {
    return code.toLowerCase();
  }
  const named: Readonly<Record<string, string>> = {
    Space: "space",
    Enter: "enter",
    Tab: "tab",
    Backspace: "backspace",
    Delete: "delete",
    ArrowUp: "arrowup",
    ArrowDown: "arrowdown",
    ArrowLeft: "arrowleft",
    ArrowRight: "arrowright",
    Home: "home",
    End: "end",
    PageUp: "pageup",
    PageDown: "pagedown",
  };
  return named[code] ?? null;
}
