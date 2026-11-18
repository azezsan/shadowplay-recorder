import js from "@eslint/js";
import svelte from "eslint-plugin-svelte";
import globals from "globals";
import ts from "typescript-eslint";

export default ts.config(
  {
    // Generated or vendored: bindings come from Rust, ui/ from shadcn-svelte.
    ignores: [
      "dist",
      "target",
      ".svelte-check",
      "src-tauri",
      "src/lib/bindings.ts",
      "src/lib/components/ui",
    ],
  },
  js.configs.recommended,
  ...ts.configs.strict,
  ...svelte.configs.recommended,
  { languageOptions: { globals: { ...globals.browser, ...globals.node } } },
  {
    files: ["**/*.svelte", "**/*.svelte.ts"],
    languageOptions: { parserOptions: { parser: ts.parser } },
  },
);
