import { describe, expect, test } from "vitest";
import { hex, hexRgb } from "./palette.ts";

describe("hex", () => {
  test("16 進を 0〜1 の色にし、不透明度を添える", () => {
    expect(hex("#ff8000")).toEqual([1, 128 / 255, 0, 1]);
    expect(hex("#000000", 0.25)).toEqual([0, 0, 0, 0.25]);
    expect(hexRgb("#1A73E8")).toEqual([26 / 255, 115 / 255, 232 / 255]);
  });

  test("書き方が違えば断る", () => {
    expect(() => hex("#fff")).toThrow();
    expect(() => hex("1a73e8")).toThrow();
  });
});
