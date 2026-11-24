import { describe, expect, it } from "vitest";
import { formatBytes, packageActionLabel, sourceLabel } from "./types";

describe("formatBytes", () => {
  it("formats zero and small values", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(512)).toBe("512 B");
    expect(formatBytes(2048)).toBe("2 KiB");
  });

  it("formats MiB and GiB", () => {
    expect(formatBytes(64 * 1024 * 1024)).toBe("64.0 MiB");
    expect(formatBytes(1.5 * 1024 ** 3)).toBe("1.50 GiB");
  });

  it("handles invalid values", () => {
    expect(formatBytes(Number.NaN)).toBe("0 B");
    expect(formatBytes(-1)).toBe("0 B");
  });
});

describe("sourceLabel", () => {
  it("labels every source", () => {
    expect(sourceLabel("system")).toBe("System");
    expect(sourceLabel("flatpak-system")).toBe("Flatpak system");
    expect(sourceLabel("flatpak-user")).toBe("Flatpak user");
  });
});

describe("packageActionLabel", () => {
  it("labels package actions", () => {
    expect(packageActionLabel("install")).toBe("Install");
    expect(packageActionLabel("remove")).toBe("Remove");
  });
});
