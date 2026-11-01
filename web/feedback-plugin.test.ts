import { mkdtempSync, readdirSync, rmSync } from "node:fs";
import { createServer, request, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import { FEEDBACK_PATH, feedbackHandler } from "./feedback-plugin.ts";

const MAX = 1024;
let root: string;
let server: Server;
let origin: string;

beforeEach(async () => {
  root = mkdtempSync(join(tmpdir(), "chizu-feedback-test-"));
  const handler = feedbackHandler(root, MAX);
  server = createServer((req, res) =>
    handler(req, res, () => {
      res.statusCode = 404;
      res.end();
    }),
  );
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  origin = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
});

afterEach(async () => {
  await new Promise((resolve) => server.close(resolve));
  rmSync(root, { recursive: true, force: true });
});

const body = JSON.stringify({ note: "メモ", screenshot: "", state: { tick: 1 }, commands: "" });

/** ヘッダーを好きに付けて送り、状態コードを返す。fetch は Origin を書き換えるので http.request で送る。 */
function post(path: string, headers: Record<string, string>, data = body): Promise<number> {
  return new Promise((resolve, reject) => {
    const req = request(`${origin}${path}`, { method: "POST", headers }, (res) => {
      res.resume();
      resolve(res.statusCode ?? 0);
    });
    req.on("error", reject);
    req.end(data);
  });
}

const json = () => ({ "Content-Type": "application/json", Origin: origin });

describe("feedbackHandler", () => {
  test("同じ localhost のページからの JSON を保存する", async () => {
    expect(await post(FEEDBACK_PATH, json())).toBe(200);
    expect(readdirSync(root)).toHaveLength(1);
  });

  test("同じ秒に続けて届いても、別の置き場所に保存する", async () => {
    const codes = await Promise.all([1, 2, 3].map(() => post(FEEDBACK_PATH, json())));
    expect(codes).toEqual([200, 200, 200]);
    expect(readdirSync(root)).toHaveLength(3);
  });

  test("ほかのページからの書き込みを弾く", async () => {
    expect(await post(FEEDBACK_PATH, { ...json(), Origin: "http://evil.example" })).toBe(403);
    expect(await post(FEEDBACK_PATH, { ...json(), Origin: "http://127.0.0.1:1" })).toBe(403);
    expect(await post(FEEDBACK_PATH, { "Content-Type": "application/json" })).toBe(403);
    expect(readdirSync(root)).toHaveLength(0);
  });

  test("JSON でない本文を弾く", async () => {
    expect(await post(FEEDBACK_PATH, { ...json(), "Content-Type": "text/plain" })).toBe(415);
    expect(readdirSync(root)).toHaveLength(0);
  });

  test("上限を超える本文を弾く", async () => {
    const big = JSON.stringify({ note: "x".repeat(MAX), screenshot: "", state: {}, commands: "" });
    expect(await post(FEEDBACK_PATH, json(), big)).toBe(413);
    // 長さを先に言わない送り方でも、読みながら上限で止める
    const chunked = { ...json(), "Transfer-Encoding": "chunked" };
    expect(await post(FEEDBACK_PATH, chunked, big)).toBe(413);
    expect(readdirSync(root)).toHaveLength(0);
  });

  test("パスが完全に一致しないものは扱わない", async () => {
    expect(await post(`${FEEDBACK_PATH}/anything`, json())).toBe(404);
    expect(await post(`${FEEDBACK_PATH}?x=1`, json())).toBe(404);
  });
});
