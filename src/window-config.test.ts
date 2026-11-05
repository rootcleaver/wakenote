import { describe, expect, it } from "vitest";
// @ts-ignore Vitest executes this source assertion in Node.
import { readFileSync, readdirSync } from "node:fs";

const config = JSON.parse(
  readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"),
);

describe("main window responsive boundary", () => {
  it("allows resizing down to the supported 480x640 quarter-screen viewport", () => {
    const mainWindow = config.app.windows[0];

    expect(mainWindow.minWidth).toBe(480);
    expect(mainWindow.minHeight).toBe(640);
    expect(mainWindow.resizable).toBe(true);
  });
});

describe("native window action permissions", () => {
  it("allows the main window's show, focus and hide calls without granting them to overlays", () => {
    const directory = new URL("../src-tauri/capabilities/", import.meta.url);
    const capabilities = readdirSync(directory)
      .filter((name: string) => name.endsWith(".json"))
      .map((name: string) => JSON.parse(readFileSync(new URL(name, directory), "utf8")));
    const permissionsFor = (label: string) => capabilities
      .filter((capability: { windows: string[] }) => capability.windows.includes(label))
      .flatMap((capability: { permissions: string[] }) => capability.permissions);
    for (const permission of ["core:window:allow-show", "core:window:allow-set-focus", "core:window:allow-hide"]) {
      expect(permissionsFor("main")).toContain(permission);
      expect(permissionsFor("overlay")).not.toContain(permission);
    }
  });
});
