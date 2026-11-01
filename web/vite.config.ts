import { defineConfig, type Plugin } from "vite";
import { feedbackPlugin } from "./feedback-plugin.ts";

// SharedArrayBuffer を使うには、ページが cross-origin isolated でなければならない
const crossOriginIsolation = {
  "Cross-Origin-Opener-Policy": "same-origin",
  "Cross-Origin-Embedder-Policy": "require-corp",
};

// scripts/play.sh が渡す、配信している版（コミットの短いハッシュと日時）を右下に小さく出す
function playVersionPlugin(version: string | undefined): Plugin {
  return {
    name: "chizu-play-version",
    transformIndexHtml() {
      if (version === undefined) {
        return [];
      }
      return [
        {
          tag: "div",
          attrs: {
            id: "play-version",
            style:
              "position:fixed;right:4px;bottom:2px;font:11px monospace;color:#aaa;pointer-events:none",
          },
          children: version,
          injectTo: "body",
        },
      ];
    },
  };
}

export default defineConfig({
  plugins: [feedbackPlugin(), playVersionPlugin(process.env.CHIZU_PLAY_VERSION)],
  server: { headers: crossOriginIsolation },
  preview: { headers: crossOriginIsolation },
  // wasm-bindgen-rayon は Worker の中からモジュールの Worker を起こすので、Worker も ES モジュールで出す
  worker: { format: "es" },
  build: { target: "es2022" },
});
