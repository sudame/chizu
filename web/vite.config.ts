import { defineConfig } from "vite";
import { feedbackPlugin } from "./feedback-plugin.ts";

// SharedArrayBuffer を使うには、ページが cross-origin isolated でなければならない
const crossOriginIsolation = {
  "Cross-Origin-Opener-Policy": "same-origin",
  "Cross-Origin-Embedder-Policy": "require-corp",
};

export default defineConfig({
  plugins: [feedbackPlugin()],
  server: { headers: crossOriginIsolation },
  preview: { headers: crossOriginIsolation },
  // wasm-bindgen-rayon は Worker の中からモジュールの Worker を起こすので、Worker も ES モジュールで出す
  worker: { format: "es" },
  build: { target: "es2022" },
});
