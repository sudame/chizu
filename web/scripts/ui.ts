// test:wasm、test:save、test:commute が共有する、画面の部品を操作する小さな関数。
// 道具の 2 段目、レイヤーのパネル、詳細の欄は、押して開くまで見えないので、開いてから押す。

import type { Page } from "playwright-core";

const MENUS: Record<string, string> = {
  road: "road",
  upgrade: "road",
  zone: "zone",
  service: "service",
  signal: "signal",
};

/** 2 段目の中にある道具の名前から、それを開く帯のボタンの名前を引く。帯に直接ある道具は null。 */
function menuOf(tool: string): string | null {
  return MENUS[tool.split("-")[0] ?? ""] ?? null;
}

/** 道具を選ぶ。2 段目の中にあるものは、閉じていれば帯のボタンで開いてから押す。 */
export async function pickTool(page: Page, tool: string): Promise<void> {
  const button = page.locator(`button[data-tool="${tool}"]`);
  const menu = menuOf(tool);
  if (menu !== null && !(await button.isVisible())) {
    await page.locator(`button[data-menu="${menu}"]`).click();
  }
  await button.click();
}

/** 上の帯の「⋯」で開く詳細の欄を、閉じていれば開く。 */
export async function openHudDetails(page: Page): Promise<void> {
  if (!(await page.locator("#hud-details").isVisible())) {
    await page.locator("#hud-toggle").click();
  }
}

/** 帯にない速度（0.25、0.5、48 倍）は詳細の欄の中にあるので、開いてから押す。 */
export async function pickSpeed(page: Page, speed: number | string): Promise<void> {
  const button = page.locator(`button[data-speed="${speed}"]`);
  if (!(await button.isVisible())) {
    await openHudDetails(page);
  }
  await button.click();
}

/** レイヤーを選ぶ。パネルが閉じていれば開いてから押す。layer は "none" かレイヤーの番号。 */
export async function chooseLayer(page: Page, layer: string): Promise<void> {
  if (!(await page.locator("#layer-panel").isVisible())) {
    await page.locator("#layers-open").click();
  }
  await page.locator(`#layers button[data-layer="${layer}"]`).click();
}
