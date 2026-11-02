import { describe, expect, test } from "vitest";
import { growthNotice, parseGrowthBlocks } from "./growth-notice.ts";

/** 区画ごとの [止まっている数, 電気, 水道] から overview の並びを作る。 */
const overview = (zones: [number, number, number][]) => {
  const flat = zones.flat();
  return [...new Array<number>(9).fill(0), ...flat, ...new Array<number>(12 - flat.length).fill(0)];
};

describe("growth notice", () => {
  test("overview から区画ごとの数を読む", () => {
    expect(
      parseGrowthBlocks(
        overview([
          [0, 0, 0],
          [3, 3, 1],
        ]),
      )[1],
    ).toEqual({ blocked: 3, noPower: 3, noWater: 1 });
  });

  test("止まっている間口がなければ知らせない", () => {
    expect(growthNotice(parseGrowthBlocks(overview([])))).toBeNull();
  });

  test("電気と水道の両方がなければ、両方を挙げて施設を置くよう促す", () => {
    const n = growthNotice(
      parseGrowthBlocks(
        overview([
          [4, 4, 4],
          [2, 2, 2],
        ]),
      ),
    );
    expect(n?.headline).toContain("電気と水道の余裕がなく");
    expect(n?.headline).toContain("住宅 4・商業 2");
    expect(n?.action).toBe("発電所と浄水場を置くか、発電所と浄水場のある道路網につなぐ");
  });

  test("電気だけが足りないときは電気だけを挙げる", () => {
    const n = growthNotice(
      parseGrowthBlocks(
        overview([
          [0, 0, 0],
          [0, 0, 0],
          [5, 5, 0],
        ]),
      ),
    );
    expect(n?.headline).toContain("電気の余裕がなく");
    expect(n?.headline).not.toContain("水道");
    expect(n?.action).toContain("発電所");
  });
});
