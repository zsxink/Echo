// ESLint flat config for the Echo desktop UI.
import js from "@eslint/js";
import globals from "globals";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist", "node_modules", "coverage"] },
  {
    files: ["**/*.{ts,tsx}"],
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    languageOptions: {
      ecmaVersion: 2022,
      globals: globals.browser,
    },
    plugins: {
      "react-hooks": reactHooks,
      "react-refresh": reactRefresh,
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      "react-refresh/only-export-components": ["warn", { allowConstantExport: true }],
      // Boundary input must be validated before reaching app state; keep `any`
      // out of the codebase (CODE_STANDARDS §6).
      "@typescript-eslint/no-explicit-any": "error",
      // Underscore-prefixed params are intentionally unused (e.g. a mock's
      // optional `args`); don't flag them as unused.
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
    },
  },
  {
    files: ["src/**/*.{ts,tsx}"],
    rules: {
      // State modules render through app/externalStore so every shared store
      // has the same immutable snapshot + subscription contract.
      "no-restricted-imports": [
        "error",
        {
          name: "react",
          importNames: ["useSyncExternalStore"],
          message: "Use useExternalStore from app/externalStore for shared state.",
        },
      ],
    },
  },
  {
    files: ["src/app/externalStore.ts"],
    rules: { "no-restricted-imports": "off" },
  },
  {
    files: ["src/features/**/*.{ts,tsx}"],
    rules: {
      // Feature internals are private. A sibling feature may only consume its
      // public `../feature` entry point, never `../feature/internal`.
      "no-restricted-imports": [
        "error",
        {
          paths: [
            {
              name: "react",
              importNames: ["useSyncExternalStore"],
              message: "Use useExternalStore from app/externalStore for shared state.",
            },
          ],
          patterns: [
            {
              group: [
                "../import/*",
                "../library/*",
                "../player/*",
                "../playlists/*",
                "../settings/*",
                "../workspace/*",
              ],
              message: "Cross-feature imports must use the feature's public index.ts entry point.",
            },
          ],
        },
      ],
      // A bare `void bridge.call(...)` discards rejected IPC commands. Use the
      // bridge's explicit, observable fireAndForget API instead.
      "no-restricted-syntax": [
        "error",
        {
          selector:
            "UnaryExpression[operator='void'] > CallExpression[callee.object.name='bridge'][callee.property.name='call']",
          message: "Use bridge.fireAndForget(...) or handle bridge.call(...) failures explicitly.",
        },
      ],
    },
  },
);
