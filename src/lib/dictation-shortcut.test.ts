import { describe, expect, it } from "vitest";
import {
  SUPPORTED_DICTATION_KEY_CODES,
  dictationShortcutFromKeyboardEvent,
  formatDictationShortcut,
  normalizeDictationShortcut,
  normalizeCycleMicrophoneShortcut,
  validateDictationHotkeys,
} from "./dictation-shortcut";
import { defaultSettings } from "./app-state";

describe("Primary microphone cycle shortcut", () => {
  it("normalizes a keyed chord and supports disabling, while rejecting modifier-only or unmodified keys", () => {
    expect(normalizeCycleMicrophoneShortcut(" Shift + Control + M ")).toBe("ctrl+shift+m");
    expect(normalizeCycleMicrophoneShortcut(" ")).toBe("");
    expect(() => normalizeCycleMicrophoneShortcut("ctrl+shift")).toThrow();
    expect(() => normalizeCycleMicrophoneShortcut("m")).toThrow();
  });
  it("rejects overlap with an enabled Dictation chord and permits disabling cycle", () => {
    const settings = { ...defaultSettings(), dictation_enabled: true, dictation_shortcut: "ctrl+shift" };
    expect(() => validateDictationHotkeys(settings)).toThrow(/Cycle Primary microphone.*overlaps/);
    expect(() => validateDictationHotkeys({ ...settings, cycle_microphone_shortcut: "" })).not.toThrow();
  });
});

function keyEvent(
  code: string,
  overrides: Partial<{
    key: string;
    ctrlKey: boolean;
    altKey: boolean;
    shiftKey: boolean;
    metaKey: boolean;
  }> = {},
) {
  return {
    code,
    key: overrides.key ?? code,
    ctrlKey: overrides.ctrlKey ?? false,
    altKey: overrides.altKey ?? false,
    shiftKey: overrides.shiftKey ?? false,
    metaKey: overrides.metaKey ?? false,
  };
}

describe("dictation shortcut helpers", () => {
  it("normalizes aliases, whitespace, case, and modifier order", () => {
    expect(normalizeDictationShortcut(" Command + SHIFT + Option + Control + D ")).toBe(
      "ctrl+alt+shift+cmd+d",
    );
  });

  it("accepts modifier chords, modified keys, physical modifiers, and single keys", () => {
    expect(normalizeDictationShortcut(" Shift + Control ")).toBe("ctrl+shift");
    expect(normalizeDictationShortcut("Control+V")).toBe("ctrl+v");
    expect(normalizeDictationShortcut("Option+Command")).toBe("alt+cmd");
    expect(normalizeDictationShortcut("Shift+Z")).toBe("shift+z");
    expect(normalizeDictationShortcut("Z")).toBe("z");
    expect(normalizeDictationShortcut("Space")).toBe("space");
    expect(normalizeDictationShortcut("LeftControl")).toBe("leftctrl");

    for (const shortcut of [
      "",
      "ctrl",
      "fn+space",
      "ctrl+a+b",
      "ctrl++d",
      "ctrl+ctrl+d",
    ]) {
      expect(() => normalizeDictationShortcut(shortcut), shortcut).toThrow();
    }
  });

  it("uses the physical key code so Option-modified characters stay stable", () => {
    expect(
      dictationShortcutFromKeyboardEvent(
        keyEvent("KeyD", { key: "∂", altKey: true }),
      ),
    ).toBe("alt+d");
    expect(dictationShortcutFromKeyboardEvent(keyEvent("F8"))).toBe("f8");
  });

  it("captures a physical modifier key and rejects unsupported keys", () => {
    expect(
      dictationShortcutFromKeyboardEvent(
        keyEvent("AltLeft", { key: "Alt", altKey: true }),
      ),
    ).toBe("leftalt");
    expect(
      dictationShortcutFromKeyboardEvent(
        keyEvent("MediaPlayPause", { altKey: true }),
      ),
    ).toBeNull();
  });

  it("captures a modifier-only chord when the second modifier is pressed", () => {
    expect(
      dictationShortcutFromKeyboardEvent(
        keyEvent("ShiftLeft", {
          key: "Shift",
          ctrlKey: true,
          shiftKey: true,
        }),
      ),
    ).toBe("ctrl+shift");
  });

  it("formats canonical shortcuts for people", () => {
    expect(formatDictationShortcut("ctrl+alt+shift+cmd+d")).toBe(
      "Control + Option + Shift + Command + D",
    );
    expect(formatDictationShortcut("alt+space")).toBe("Option + Space");
    expect(formatDictationShortcut("ctrl+shift")).toBe("Control + Shift");
    expect(formatDictationShortcut("leftctrl")).toBe("Left Control");
  });

  it("emits only canonical shortcuts accepted by its own validator", () => {
    for (const code of SUPPORTED_DICTATION_KEY_CODES) {
      const event = keyEvent(code);
      const shortcut = dictationShortcutFromKeyboardEvent(event);
      expect(shortcut, code).not.toBeNull();
      expect(normalizeDictationShortcut(shortcut ?? ""), code).toBe(shortcut);
    }
  });
});
