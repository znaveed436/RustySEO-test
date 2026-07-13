# Required credential cleanup

Before publishing another build:

1. Remove the hard-coded `const API_KEY` from `src-tauri/src/gemini.rs`.
2. Remove any Google OAuth client secret and API keys committed in Rust source, `.env`, or README files.
3. Revoke/rotate every exposed credential in Google Cloud / Google AI Studio.
4. Keep runtime keys only in RustySEO's application config or GitHub Actions secrets.
5. Ensure `.env` is ignored and remove it from Git history if it was committed.

The supplied `image_alt_suggester.rs` reads the Gemini key through
`crate::gemini::get_gemini_config()` and does not embed a key.
