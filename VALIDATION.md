# Validation performed

- Parsed all three Rust source files with the Rust tree-sitter grammar: no syntax errors.
- Parsed `.github/workflows/main.yml` as YAML: valid.
- Parsed `src-tauri/tauri.conf.json` as JSON: valid.
- Type-checked `app/utils/advancedSeoApi.ts` in TypeScript strict mode with a Tauri `invoke` declaration: passed.
- Regenerated `package-lock.json` and verified it with:
  `npm ci --package-lock-only --legacy-peer-deps --ignore-scripts --no-audit --no-fund`.

A full `cargo check` could not be executed in the artifact environment because a Rust toolchain was not installed. The supplied GitHub workflow runs both `cargo check --locked` and `cargo test --locked` before building the installer, so GitHub Actions is the final compile gate.
