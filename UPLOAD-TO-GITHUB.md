# Upload these changes through GitHub's web editor

Use the `JulesTest` branch, because its current `main.rs` already declares and
registers the six advanced-feature commands.

## 1. Create or replace the Rust modules

Upload these files to `src-tauri/src/`:

- `a11y_audit.rs` — new file
- `content_gap_analyzer.rs` — new file
- `image_alt_suggester.rs` — replace the existing heuristic implementation

In GitHub: open the target folder, choose **Add file → Upload files**, select the
three `.rs` files, and commit directly to `JulesTest`.

## 2. Confirm `main.rs`

Open `src-tauri/src/main.rs` and confirm the declarations and command
registrations listed in `main.rs-changes.txt` exist exactly once. Your current
JulesTest branch already contains them, so do not replace the whole file.

## 3. Replace the Windows workflow

Replace `.github/workflows/main.yml` with the supplied file. It:

- runs only on Windows;
- uses the committed Tauri CLI through `npx --no-install`;
- validates Rust with `cargo check` and `cargo test`;
- builds only the NSIS `*-setup.exe` installer;
- uploads the EXE as an Actions artifact;
- creates a release only for `v*` tags.

## 4. Replace Tauri bundle configuration

Replace `src-tauri/tauri.conf.json`. The only intentional bundle change is:

```json
"targets": ["nsis"]
```

## 5. Replace `package-lock.json`

The repository's existing lockfile is out of sync with `package.json` for
Next.js. Upload the supplied `package-lock.json`; otherwise `npm ci` fails before
the Rust build starts.

## 6. Optional frontend API bridge

Upload `app/utils/advancedSeoApi.ts`. It provides typed Tauri `invoke()` wrappers
for all six commands. It does not add visible tabs by itself.

## 7. Security cleanup

Follow `SECURITY-NOTICE.md`, revoke exposed keys, remove committed secrets, and
never paste API keys into source files.

## 8. Run the workflow

Open **Actions → Build Windows EXE → Run workflow**, select `JulesTest`, and run
it. A successful run uploads `RustySEO-Windows-Installer` containing a
`*-setup.exe` file.

For a GitHub Release, create a tag such as `v0.3.10` after the branch build
passes.
