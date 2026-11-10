# Release notes

For each meaningful change, update the current app version's entry in `CHANGELOG.json` as part of the same work. Include a concise user-facing `brief` and an array of `details`; document behavior changes, fixes, build tooling, and other substantive changes. Do not include credentials or sensitive implementation details. Repository-only changes (README, AGENTS.md, and similar housekeeping) are not app changes and must not be logged.

Each current change also needs a unique `id`, `category` (`Added`, `Fixed`, `Changed`, `Security`, or `Tooling`), and exact repository-relative `files`. Check `development.version`, `previousVersion`, and `baselineCommit` before writing notes. Do not infer published releases from version numbers or Git commits; historical snapshots must be identified as such.

After finishing edits and tests, run `node scripts/changelog.mjs checkpoint`, then `node scripts/changelog.mjs check`. The checkpoint requires file coverage and records fingerprints; check rejects undocumented files and subsequent changes. Write meaningful explanations before checkpointing rather than merely adding paths. Generated guide copies are maintained by `node scripts/changelog.mjs sync`; edit `scripts/templates/guide.html` and `scripts/changelog-view.js`, not their generated copies.

Preserve prior release entries. When changing the app version, add an entry matching both `app/package.json` and `app/src-tauri/tauri.conf.json`. The desktop build validates this entry and uses it for the bundled update dialog and updater manifest. See the release workflow in `RELEASING.md`.

Version bumps must ask Y/N before transferring logged development notes. Yes moves unpublished notes to the new version and leaves a superseded draft marker; No keeps notes on the old version. Never transfer published release notes or silently choose a transfer for the user. The version script prompts before any version files change.
