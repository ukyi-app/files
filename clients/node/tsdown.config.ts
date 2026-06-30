import { defineConfig } from "tsdown";

export default defineConfig({
  entry: ["src/index.ts"],
  format: ["esm", "cjs"],
  dts: true,
  clean: true,
  // 웹 표준(fetch/Response/ReadableStream)만 사용 → node·browser 양쪽 동작
  platform: "neutral",
});
