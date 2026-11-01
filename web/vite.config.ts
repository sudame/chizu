import { defineConfig } from "vite";

// SharedArrayBuffer を使うには、ページが cross-origin isolated でなければならない
const crossOriginIsolation = {
  "Cross-Origin-Opener-Policy": "same-origin",
  "Cross-Origin-Embedder-Policy": "require-corp",
};

export default defineConfig({
  server: { headers: crossOriginIsolation },
  preview: { headers: crossOriginIsolation },
  // wasm-bindgen-rayon は Worker の中からモジュールの Worker を起こすので、Worker も ES モジュールで出す
  worker: { format: "es" },
  build: { target: "es2022" },
});
